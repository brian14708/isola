# pyright: reportPrivateUsage=false

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterable, Awaitable, Callable
from dataclasses import dataclass, field
from os import PathLike, fspath
from typing import TYPE_CHECKING, Literal, cast
from typing_extensions import Self, TypedDict, Unpack

import httpx

from isola._isola import _ContextCore, _StreamCore

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Iterable

    from isola._isola import _SandboxCore

JsonScalar = bool | int | float | str | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
RuntimeName = Literal["python", "js"]
BytesLike = bytes | bytearray | memoryview
Pathish = str | PathLike[str]
HttpBodyOut = BytesLike | AsyncIterable[BytesLike] | None
HostcallHandler = Callable[[JsonValue], Awaitable[object]]
Hostcalls = dict[str, HostcallHandler]


@dataclass(slots=True)
class ResultEvent:
    data: JsonValue


@dataclass(slots=True)
class EndEvent:
    data: JsonValue | None


@dataclass(slots=True)
class StdoutEvent:
    data: str


@dataclass(slots=True)
class StderrEvent:
    data: str


@dataclass(slots=True)
class ErrorEvent:
    data: str


@dataclass(slots=True)
class LogEvent:
    data: str


Event = ResultEvent | EndEvent | StdoutEvent | StderrEvent | ErrorEvent | LogEvent


@dataclass(slots=True)
class HttpRequest:
    method: str
    url: str
    headers: dict[str, str]
    body: bytes | None


@dataclass(slots=True)
class HttpResponse:
    status: int
    headers: dict[str, str] = field(default_factory=dict)
    body: HttpBodyOut = None


async def _default_httpx_handler(request: HttpRequest) -> HttpResponse:
    client = httpx.AsyncClient()
    try:
        outbound_request = client.build_request(
            request.method, request.url, headers=request.headers, content=request.body
        )
        response = await client.send(outbound_request, stream=True)
    except Exception:
        await client.aclose()
        raise

    async def _stream_body() -> AsyncIterable[bytes]:
        try:
            async for chunk in response.aiter_bytes():
                yield chunk
        finally:
            await response.aclose()
            await client.aclose()

    return HttpResponse(
        status=response.status_code, headers=dict(response.headers), body=_stream_body()
    )


@dataclass(slots=True)
class MountConfig:
    host: Pathish
    guest: str
    dir_perms: Literal["read", "write", "read-write"] = "read"
    file_perms: Literal["read", "write", "read-write"] = "read"

    def to_dict(self) -> dict[str, str]:
        return {
            "host": _normalize_path(self.host, key="host"),
            "guest": self.guest,
            "dir_perms": self.dir_perms,
            "file_perms": self.file_perms,
        }


class TemplateConfig(TypedDict, total=False):
    runtime_path: Pathish | None
    cache_dir: Pathish | None
    max_memory: int | None
    prelude: str | None
    runtime_lib_dir: Pathish | None
    mounts: list[MountConfig] | None
    env: dict[str, str]


class SandboxConfig(TypedDict, total=False):
    max_memory: int | None
    mounts: list[MountConfig] | None
    env: dict[str, str]
    http_handler: Callable[[HttpRequest], Awaitable[object]] | None
    hostcalls: Hostcalls | None


@dataclass(slots=True)
class Arg:
    value: object
    name: str | None = None


class StreamArg:
    def __init__(
        self,
        core: _StreamCore,
        *,
        name: str | None = None,
        source: AsyncIterable[object] | None = None,
        producer_task: asyncio.Task[None] | None = None,
    ) -> None:
        self._core = core
        self.name = name
        self._source = source
        self._producer_task = producer_task

    @property
    def stream_core(self) -> _StreamCore:
        return self._core

    @property
    def producer_task(self) -> asyncio.Task[None] | None:
        return self._producer_task

    def start_producer(self) -> asyncio.Task[None] | None:
        if self._producer_task is not None or self._source is None:
            return self._producer_task

        source = self._source
        self._source = None

        async def _produce() -> None:
            try:
                async for item in source:
                    payload = _to_json(item)
                    await self._core.push_json_async(payload)
            finally:
                self._core.end()

        self._producer_task = asyncio.create_task(_produce())
        return self._producer_task

    @classmethod
    def from_async_iterable(
        cls,
        values: AsyncIterable[object],
        *,
        name: str | None = None,
        capacity: int = 1024,
    ) -> StreamArg:
        core = _StreamCore(capacity)
        return cls(core, name=name, source=values)

    @classmethod
    def from_iterable(
        cls, values: Iterable[object], *, name: str | None = None, capacity: int = 1024
    ) -> StreamArg:
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            buffered = list(values)
            core = _StreamCore(max(capacity, len(buffered), 1))
            for item in buffered:
                core.push_json(_to_json(item))
            core.end()
            return cls(core, name=name)

        async def _iterate() -> AsyncIterable[object]:
            for item in values:
                await asyncio.sleep(0)
                yield item

        return cls.from_async_iterable(_iterate(), name=name, capacity=capacity)


RunArg = Arg | StreamArg | JsonValue


def _normalize_mounts(
    mounts: object, *, key: str = "mounts"
) -> list[dict[str, str]] | None:
    if mounts is None:
        return None
    if not isinstance(mounts, list):
        msg = f"{key} must be a list[MountConfig] or None"
        raise TypeError(msg)

    mount_items = cast("list[object]", mounts)
    encoded: list[dict[str, str]] = []
    for mount_obj in mount_items:
        if not isinstance(mount_obj, MountConfig):
            msg = f"{key} entries must be MountConfig"
            raise TypeError(msg)
        encoded.append(mount_obj.to_dict())
    return encoded


def _normalize_path(value: object, *, key: str) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, bytes):
        msg = f"{key} must be str | os.PathLike[str], not bytes"
        raise TypeError(msg)
    if isinstance(value, PathLike):
        path_like_value = cast("PathLike[str] | PathLike[bytes]", value)
        raw_path = fspath(path_like_value)
        if isinstance(raw_path, bytes):
            msg = f"{key} must be str | os.PathLike[str], not bytes"
            raise TypeError(msg)
        return raw_path
    msg = f"{key} must be str | os.PathLike[str]"
    raise TypeError(msg)


def _normalize_optional_path(value: object, *, key: str) -> str | None:
    if value is None:
        return None
    return _normalize_path(value, key=key)


def _configure_core(
    core: _ContextCore | _SandboxCore, patch: dict[str, object]
) -> None:
    if patch:
        core.configure_json(json.dumps(patch, separators=(",", ":")))


class SandboxManager:
    def __init__(self) -> None:
        self._core = _ContextCore()

    async def compile_template(
        self,
        runtime: RuntimeName,
        *,
        version: str | None = None,
        **kwargs: Unpack[TemplateConfig],
    ) -> SandboxTemplate:
        runtime_path = kwargs.pop("runtime_path", None)

        if runtime_path is None:
            from isola._runtime import resolve_runtime  # noqa: PLC0415

            defaults = await resolve_runtime(runtime, version=version)
            resolved: dict[str, object] = {**defaults, **kwargs}
        else:
            resolved = dict(kwargs)
            resolved["runtime_path"] = runtime_path

        actual_runtime_path = resolved.pop("runtime_path", None)
        if actual_runtime_path is None:
            msg = "runtime_path must be provided or resolvable via auto-download"
            raise ValueError(msg)

        patch: dict[str, object] = dict(resolved)
        if "cache_dir" not in patch or patch["cache_dir"] is None:
            from isola._runtime import _cache_base  # noqa: PLC0415

            patch["cache_dir"] = str(_cache_base() / "isola" / "cache")
        if "cache_dir" in patch:
            patch["cache_dir"] = _normalize_optional_path(
                patch["cache_dir"], key="cache_dir"
            )
        if "runtime_lib_dir" in patch:
            patch["runtime_lib_dir"] = _normalize_optional_path(
                patch["runtime_lib_dir"], key="runtime_lib_dir"
            )
        if "mounts" in patch:
            patch["mounts"] = _normalize_mounts(patch["mounts"])
        _configure_core(self._core, patch)
        normalized_runtime_path = _normalize_path(
            actual_runtime_path, key="runtime_path"
        )
        await self._core.initialize_template(normalized_runtime_path, runtime)
        return SandboxTemplate(self._core)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        self.close()

    def close(self) -> None:
        self._core.close()


class SandboxTemplate:
    def __init__(self, core: _ContextCore) -> None:
        self._core = core

    async def create(self, **kwargs: Unpack[SandboxConfig]) -> Sandbox:
        core = await self._core.instantiate()
        sandbox = Sandbox(core)

        patch: dict[str, object] = {}
        if "max_memory" in kwargs:
            patch["max_memory"] = kwargs["max_memory"]
        if "mounts" in kwargs:
            patch["mounts"] = _normalize_mounts(kwargs["mounts"])
        if "env" in kwargs:
            patch["env"] = kwargs["env"]
        _configure_core(sandbox._core, patch)  # noqa: SLF001

        hostcalls = kwargs.get("hostcalls")
        http_handler = kwargs.get("http_handler", _default_httpx_handler)
        sandbox._set_hostcalls(hostcalls)  # noqa: SLF001
        sandbox._set_http_handler(http_handler)  # noqa: SLF001
        return sandbox


class Sandbox:
    def __init__(self, core: _SandboxCore) -> None:
        self._core = core
        self._stream_dispatches: dict[int, Callable[[str, str | None], None]] = {}
        self._next_stream_dispatch_id = 0
        self._hostcall_handler_dispatch: Callable[[str, str], Awaitable[str]] | None = (
            None
        )
        self._http_handler_dispatch: (
            Callable[
                [str, str, dict[str, str], bytes | None],
                Awaitable[tuple[int, dict[str, str], str, object]],
            ]
            | None
        ) = None

    def _refresh_core_callback(self) -> None:
        if not self._stream_dispatches:
            self._core.set_callback(None)
            return

        def _dispatch(kind: str, data: str | None) -> None:
            # Stream listeners can change while events are emitted.
            stream_dispatches = tuple(self._stream_dispatches.values())
            for stream_dispatch in stream_dispatches:
                stream_dispatch(kind, data)

        self._core.set_callback(_dispatch)

    def _set_hostcalls(self, hostcalls: Hostcalls | None) -> None:
        if hostcalls is None:
            self._hostcall_handler_dispatch = None
            self._core.set_hostcall_handler(None, None)
            return

        loop = asyncio.get_running_loop()
        dispatch_hostcalls = dict(hostcalls)

        async def _dispatch(call_type: str, payload_json: str) -> str:
            payload = cast("JsonValue", json.loads(payload_json))
            handler = dispatch_hostcalls.get(call_type)
            if handler is None:
                msg = f"unsupported hostcall: {call_type}"
                raise ValueError(msg)
            result = await handler(payload)
            return _to_json(result)

        self._hostcall_handler_dispatch = _dispatch
        self._core.set_hostcall_handler(_dispatch, loop)

    def _set_http_handler(
        self, handler: Callable[[HttpRequest], Awaitable[object]] | None
    ) -> None:
        if handler is None:
            self._http_handler_dispatch = None
            self._core.set_http_handler(None, None)
            return

        loop = asyncio.get_running_loop()

        async def _dispatch(
            method: str, url: str, headers: dict[str, str], body: bytes | None
        ) -> tuple[int, dict[str, str], str, object]:
            request = HttpRequest(
                method=method, url=url, headers=dict(headers), body=body
            )
            response: object = await handler(request)
            if not isinstance(response, HttpResponse):
                msg = "http handler must return HttpResponse"
                raise TypeError(msg)
            body_mode, body_payload = _normalize_http_response_body(response.body)
            return (response.status, dict(response.headers), body_mode, body_payload)

        self._http_handler_dispatch = _dispatch
        self._core.set_http_handler(_dispatch, loop)

    async def load_script(self, code: str) -> None:
        await self._core.load_script(code)

    async def run(
        self, name: str, args: list[RunArg] | None = None
    ) -> JsonValue | None:
        final: JsonValue | None = None
        async for event in self.run_stream(name, args):
            if isinstance(event, EndEvent):
                final = event.data
        return final

    async def _run_operation(self, name: str, args: list[RunArg] | None = None) -> None:
        encoded_args, producers = _encode_args(args)
        operation = self._core.run(name, encoded_args)

        try:
            await operation
        except BaseException:
            for producer in producers:
                producer.cancel()
            if producers:
                await asyncio.gather(*producers, return_exceptions=True)
            raise

        if producers:
            await asyncio.gather(*producers)

    async def run_stream(
        self, name: str, args: list[RunArg] | None = None
    ) -> AsyncIterator[Event]:
        queue: asyncio.Queue[Event] = asyncio.Queue()
        loop = asyncio.get_running_loop()
        pending_dispatches = 0
        operation_finished = False
        dispatches_drained: asyncio.Future[None] = loop.create_future()

        def _enqueue(event: Event) -> None:
            nonlocal pending_dispatches
            queue.put_nowait(event)
            pending_dispatches -= 1
            if (
                operation_finished
                and pending_dispatches == 0
                and not dispatches_drained.done()
            ):
                dispatches_drained.set_result(None)

        def _dispatch(kind: str, data: str | None) -> None:
            nonlocal pending_dispatches
            event: Event
            if kind == "result_json":
                if data is None:
                    return
                event = ResultEvent(data=cast("JsonValue", json.loads(data)))
            elif kind == "end_json":
                event = EndEvent(
                    data=None if data is None else cast("JsonValue", json.loads(data))
                )
            elif kind == "stdout":
                if data is None:
                    return
                event = StdoutEvent(data=data)
            elif kind == "stderr":
                if data is None:
                    return
                event = StderrEvent(data=data)
            elif kind == "error":
                if data is None:
                    return
                event = ErrorEvent(data=data)
            elif kind == "log":
                if data is None:
                    return
                event = LogEvent(data=data)
            else:
                return
            pending_dispatches += 1
            loop.call_soon_threadsafe(_enqueue, event)

        stream_dispatch_id = self._next_stream_dispatch_id
        self._next_stream_dispatch_id += 1
        self._stream_dispatches[stream_dispatch_id] = _dispatch
        self._refresh_core_callback()
        run_task = asyncio.create_task(self._run_operation(name, args))

        try:
            while True:
                get_event_task: asyncio.Task[Event] = asyncio.create_task(queue.get())
                done, _ = await asyncio.wait(
                    {run_task, get_event_task}, return_when=asyncio.FIRST_COMPLETED
                )

                if get_event_task in done:
                    yield get_event_task.result()
                    continue

                get_event_task.cancel()
                await asyncio.gather(get_event_task, return_exceptions=True)

                operation_finished = True
                if pending_dispatches > 0:
                    await asyncio.shield(dispatches_drained)

                while not queue.empty():
                    yield queue.get_nowait()

                await run_task
                break
        finally:
            self._stream_dispatches.pop(stream_dispatch_id, None)
            self._refresh_core_callback()
            if not run_task.done():
                run_task.cancel()
                await asyncio.gather(run_task, return_exceptions=True)

    async def __aenter__(self) -> Self:
        await self._core.start()
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.aclose()

    def close(self) -> None:
        self._stream_dispatches.clear()
        self._hostcall_handler_dispatch = None
        self._http_handler_dispatch = None
        self._core.close()

    async def aclose(self) -> None:
        self._stream_dispatches.clear()
        self._hostcall_handler_dispatch = None
        self._http_handler_dispatch = None
        self._core.close()


def _to_json(value: object) -> str:
    return json.dumps(value, separators=(",", ":"))


def _encode_args(
    args: list[RunArg] | None,
) -> tuple[list[tuple[str, str | None, object]], list[asyncio.Task[None]]]:
    if args is None:
        return [], []

    encoded: list[tuple[str, str | None, object]] = []
    producers: list[asyncio.Task[None]] = []
    stream_args: list[StreamArg] = []

    for arg in args:
        if isinstance(arg, Arg):
            encoded.append(("json", arg.name, _to_json(arg.value)))
            continue

        if isinstance(arg, StreamArg):
            encoded.append(("stream", arg.name, arg.stream_core))
            stream_args.append(arg)
            continue

        encoded.append(("json", None, _to_json(arg)))

    for stream_arg in stream_args:
        producer = stream_arg.start_producer()
        if producer is not None:
            producers.append(producer)

    return encoded, producers


def _normalize_http_response_body(body: object) -> tuple[str, object]:
    if body is None:
        return ("none", None)

    if isinstance(body, bytes):
        return ("bytes", body)
    if isinstance(body, bytearray):
        return ("bytes", bytes(body))
    if isinstance(body, memoryview):
        return ("bytes", body.tobytes())

    if not isinstance(body, AsyncIterable):
        msg = "http response body must be bytes, AsyncIterable[bytes], or None"
        raise TypeError(msg)

    async def _stream_body(source: AsyncIterable[object]) -> AsyncIterable[bytes]:
        async for chunk in source:
            if isinstance(chunk, bytes):
                yield chunk
                continue
            if isinstance(chunk, bytearray):
                yield bytes(chunk)
                continue
            if isinstance(chunk, memoryview):
                yield chunk.tobytes()
                continue
            msg = "http response stream chunks must be bytes-like"
            raise TypeError(msg)

    source = cast("AsyncIterable[object]", body)
    return ("stream", _stream_body(source))
