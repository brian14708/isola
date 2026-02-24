# pyright: reportPrivateUsage=false

from __future__ import annotations

import asyncio
import inspect
import json
import math
from collections.abc import AsyncIterable
from dataclasses import dataclass, field
from os import PathLike, fspath
from typing import TYPE_CHECKING, Literal, cast
from typing_extensions import Self, TypedDict, Unpack

import httpx

from isola._isola import _ContextCore, _StreamCore

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable, Iterable

    from isola._isola import _SandboxCore

JsonScalar = bool | int | float | str | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
EventKind = Literal["result", "end", "stdout", "stderr", "error", "log"]
BytesLike = bytes | bytearray | memoryview
Pathish = str | PathLike[str]
HttpBodyOut = BytesLike | AsyncIterable[BytesLike] | None
_EVENT_KIND_MAP = {"result_json": "result", "end_json": "end"}


@dataclass(slots=True)
class Event:
    kind: EventKind
    data: str | None


@dataclass(slots=True)
class RunResult:
    results: list[JsonValue]
    final: JsonValue | None
    stdout: list[str]
    stderr: list[str]
    logs: list[str]
    errors: list[str]


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


@dataclass(slots=True)
class SandboxConfig:
    max_memory: int | None = None
    timeout: float | None = None
    mounts: list[MountConfig] = field(default_factory=list)
    env: dict[str, str] = field(default_factory=dict)

    def to_patch(self) -> _SandboxConfigureInput:
        return {
            "max_memory": self.max_memory,
            "timeout": self.timeout,
            "mounts": self.mounts,
            "env": self.env,
        }


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
        producer_task: asyncio.Task[None] | None = None,
    ) -> None:
        self._core = core
        self.name = name
        self._producer_task = producer_task

    @property
    def stream_core(self) -> _StreamCore:
        return self._core

    @property
    def producer_task(self) -> asyncio.Task[None] | None:
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

        async def _produce() -> None:
            try:
                async for item in values:
                    payload = _to_json(item)
                    await core.push_json_async(payload)
            finally:
                core.end()

        producer_task = asyncio.create_task(_produce())
        return cls(core, name=name, producer_task=producer_task)

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


class _ContextConfigurePatch(TypedDict, total=False):
    cache_dir: Pathish | None
    max_memory: int | None
    prelude: str | None
    runtime_lib_dir: Pathish | None
    mounts: list[MountConfig] | None
    env: dict[str, str]


class _SandboxConfigureInput(TypedDict, total=False):
    max_memory: int | None
    timeout: float | None
    mounts: list[MountConfig] | None
    env: dict[str, str]


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


def _timeout_seconds_to_ms(value: object) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        msg = "timeout must be float | None"
        raise TypeError(msg)
    timeout = float(value)
    if not math.isfinite(timeout):
        msg = "timeout must be finite"
        raise ValueError(msg)
    if timeout <= 0:
        msg = "timeout must be greater than 0"
        raise ValueError(msg)
    timeout_ms = math.ceil(timeout * 1000)
    if timeout_ms <= 0:
        msg = "timeout must be at least 0.001 seconds"
        raise ValueError(msg)
    return timeout_ms


def _configure_core(
    core: _ContextCore | _SandboxCore, patch: dict[str, object]
) -> None:
    if patch:
        core.configure_json(json.dumps(patch, separators=(",", ":")))


class Context:
    def __init__(self, core: _ContextCore) -> None:
        self._core = core

    @classmethod
    def create(cls) -> Context:
        core = _ContextCore()
        return cls(core)

    def configure(self, **kwargs: Unpack[_ContextConfigurePatch]) -> None:
        patch = dict(kwargs)
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

    async def initialize_template(self, runtime_path: Pathish) -> None:
        normalized_runtime_path = _normalize_path(runtime_path, key="runtime_path")
        await self._core.initialize_template(normalized_runtime_path)

    async def instantiate(self, *, config: SandboxConfig | None = None) -> Sandbox:
        core = await self._core.instantiate()
        sandbox = Sandbox(core)
        if config is not None:
            sandbox.configure(**config.to_patch())
        return sandbox

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        self.close()

    def close(self) -> None:
        self._core.close()


class Sandbox:
    def __init__(self, core: _SandboxCore) -> None:
        self._core = core
        self._event_dispatch: Callable[[str, str | None], None] | None = None
        self._http_handler_dispatch: (
            Callable[
                [str, str, dict[str, str], bytes | None],
                Awaitable[tuple[int, dict[str, str], str, object]],
            ]
            | None
        ) = None
        self._pending_callback_tasks: set[asyncio.Task[None]] = set()
        self.set_http_handler(_default_httpx_handler)

    def configure(self, **kwargs: Unpack[_SandboxConfigureInput]) -> None:
        patch = dict(kwargs)
        if "timeout" in patch:
            timeout_value = patch.pop("timeout")
            timeout_ms = _timeout_seconds_to_ms(timeout_value)
            patch["timeout_ms"] = timeout_ms
            self._http_handler_timeout_seconds = (
                None if timeout_ms is None else timeout_ms / 1000
            )
        if "mounts" in patch:
            patch["mounts"] = _normalize_mounts(patch["mounts"])
        _configure_core(self._core, patch)

    def set_callback(
        self, callback: Callable[[Event], Awaitable[None] | None] | None
    ) -> None:
        if callback is None:
            self._event_dispatch = None
            self._core.set_callback(None)
            return

        loop = asyncio.get_running_loop()

        def _dispatch(kind: str, data: str | None) -> None:
            mapped_kind = _EVENT_KIND_MAP.get(kind, kind)
            event = Event(kind=cast("EventKind", mapped_kind), data=data)

            def _invoke() -> None:
                try:
                    outcome = callback(event)
                except Exception as exc:  # noqa: BLE001  # pragma: no cover - loop handler path
                    context: dict[str, object] = {
                        "message": "isola callback raised synchronously",
                        "exception": exc,
                    }
                    loop.call_exception_handler(context)
                    return
                if inspect.isawaitable(outcome):
                    future = asyncio.ensure_future(outcome)
                    self._pending_callback_tasks.add(future)

                    def _on_done(task: asyncio.Task[None]) -> None:
                        self._pending_callback_tasks.discard(task)
                        if task.cancelled():
                            return
                        exc = task.exception()
                        if exc is not None:  # pragma: no cover - loop handler path
                            context: dict[str, object] = {
                                "message": "isola callback task failed",
                                "exception": exc,
                                "task": task,
                            }
                            loop.call_exception_handler(context)

                    future.add_done_callback(_on_done)

            loop.call_soon_threadsafe(_invoke)

        self._event_dispatch = _dispatch
        self._core.set_callback(_dispatch)

    def set_http_handler(
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
            timeout = self._http_handler_timeout_seconds
            if timeout is None:
                response: object = await handler(request)
            else:
                response = await asyncio.wait_for(handler(request), timeout=timeout)
            if not isinstance(response, HttpResponse):
                msg = "http handler must return HttpResponse"
                raise TypeError(msg)
            body_mode, body_payload = _normalize_http_response_body(response.body)
            return (response.status, dict(response.headers), body_mode, body_payload)

        self._http_handler_dispatch = _dispatch
        self._core.set_http_handler(_dispatch, loop)

    async def start(self) -> None:
        await self._core.start()

    async def load_script(self, code: str) -> None:
        await self._core.load_script(code)

    async def run(self, name: str, args: list[RunArg] | None = None) -> RunResult:
        results: list[JsonValue] = []
        final: JsonValue | None = None
        stdout: list[str] = []
        stderr: list[str] = []
        logs: list[str] = []
        errors: list[str] = []

        async for event in self.run_stream(name, args):
            if event.kind == "result":
                if event.data is not None:
                    results.append(cast("JsonValue", json.loads(event.data)))
                continue

            if event.kind == "end":
                final = (
                    None
                    if event.data is None
                    else cast("JsonValue", json.loads(event.data))
                )
                continue

            if event.kind == "stdout":
                if event.data is not None:
                    stdout.append(event.data)
                continue

            if event.kind == "stderr":
                if event.data is not None:
                    stderr.append(event.data)
                continue

            if event.kind == "log":
                if event.data is not None:
                    logs.append(event.data)
                continue

            if event.data is not None:
                errors.append(event.data)

        return RunResult(
            results=results,
            final=final,
            stdout=stdout,
            stderr=stderr,
            logs=logs,
            errors=errors,
        )

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
        previous_dispatch = self._event_dispatch

        def _dispatch(kind: str, data: str | None) -> None:
            mapped_kind = _EVENT_KIND_MAP.get(kind, kind)
            event = Event(kind=cast("EventKind", mapped_kind), data=data)
            loop.call_soon_threadsafe(queue.put_nowait, event)
            if previous_dispatch is not None:
                previous_dispatch(kind, data)

        self._event_dispatch = _dispatch
        self._core.set_callback(_dispatch)
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

                while not queue.empty():
                    yield queue.get_nowait()

                await run_task
                break
        finally:
            if self._event_dispatch is _dispatch:
                self._event_dispatch = previous_dispatch
                self._core.set_callback(previous_dispatch)
            if not run_task.done():
                run_task.cancel()
                await asyncio.gather(run_task, return_exceptions=True)

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.aclose()

    def close(self) -> None:
        self._cancel_pending_callback_tasks()
        self._event_dispatch = None
        self._http_handler_dispatch = None
        self._core.close()

    async def aclose(self) -> None:
        pending = self._cancel_pending_callback_tasks()
        if pending:
            await asyncio.gather(*pending, return_exceptions=True)
        self._event_dispatch = None
        self._http_handler_dispatch = None
        self._core.close()

    def _cancel_pending_callback_tasks(self) -> tuple[asyncio.Task[None], ...]:
        pending = tuple(self._pending_callback_tasks)
        for task in pending:
            task.cancel()
        self._pending_callback_tasks.clear()
        return pending


def _to_json(value: object) -> str:
    return json.dumps(value, separators=(",", ":"))


def _encode_args(
    args: list[RunArg] | None,
) -> tuple[list[tuple[str, str | None, object]], list[asyncio.Task[None]]]:
    if args is None:
        return [], []

    encoded: list[tuple[str, str | None, object]] = []
    producers: list[asyncio.Task[None]] = []

    for arg in args:
        if isinstance(arg, Arg):
            encoded.append(("json", arg.name, _to_json(arg.value)))
            continue

        if isinstance(arg, StreamArg):
            encoded.append(("stream", arg.name, arg.stream_core))
            if arg.producer_task is not None:
                producers.append(arg.producer_task)
            continue

        encoded.append(("json", None, _to_json(arg)))

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
