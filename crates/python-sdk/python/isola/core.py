from __future__ import annotations

# pyright: reportPrivateUsage=false
import asyncio
import inspect
import json
import os
import shutil
import sys
import tarfile
import tempfile
import urllib.request
from collections.abc import AsyncIterable
from dataclasses import dataclass, field
from functools import partial
from pathlib import Path
from typing import TYPE_CHECKING, Literal, TypeVar, cast

from isola._isola import IsolaError, _ContextCore, _StreamCore

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable, Iterable
    from contextlib import AbstractAsyncContextManager

    from isola._isola import _RunResultCore, _SandboxCore

JsonScalar = bool | int | float | str | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]
EventKind = Literal["result_json", "end_json", "stdout", "stderr", "error", "log"]
HttpBodyOut = bytes | AsyncIterable[bytes] | None
T = TypeVar("T")

_UNSET = object()
_RUNTIME_BUNDLE_URL = (
    "https://github.com/brian14708/isola/releases/download/latest/isola-python.tar.gz"
)


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
class HttpRequestData:
    method: str
    url: str
    headers: dict[str, str]
    body: bytes | None


@dataclass(slots=True)
class HttpResponseData:
    status: int
    headers: dict[str, str] = field(default_factory=dict)
    body: HttpBodyOut = None


@dataclass(slots=True)
class MountConfig:
    host: str
    guest: str
    dir_perms: Literal["read", "write", "read-write"] = "read"
    file_perms: Literal["read", "write", "read-write"] = "read"

    def to_dict(self) -> dict[str, str]:
        return {
            "host": self.host,
            "guest": self.guest,
            "dir_perms": self.dir_perms,
            "file_perms": self.file_perms,
        }


@dataclass(slots=True)
class SandboxConfig:
    max_memory: int | None = None
    timeout_ms: int | None = 30_000
    mounts: list[MountConfig] = field(default_factory=list)
    env: dict[str, str] = field(default_factory=dict)

    def to_patch(self) -> dict[str, object]:
        return {
            "max_memory": self.max_memory,
            "timeout_ms": self.timeout_ms,
            "mounts": [mount.to_dict() for mount in self.mounts],
            "env": self.env,
        }


@dataclass(slots=True)
class JsonArg:
    value: object
    name: str | None = None


class JsonStreamArg:
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
    ) -> JsonStreamArg:
        core = _StreamCore(capacity)

        async def _produce() -> None:
            try:
                async for item in values:
                    payload = _to_json(item)
                    operation = partial(core.push_json, payload, blocking=True)
                    await asyncio.to_thread(operation)
            finally:
                await asyncio.to_thread(core.end)

        producer_task = asyncio.create_task(_produce())
        return cls(core, name=name, producer_task=producer_task)

    @classmethod
    def from_iterable(
        cls, values: Iterable[object], *, name: str | None = None, capacity: int = 1024
    ) -> JsonStreamArg:
        core = _StreamCore(capacity)
        for item in values:
            core.push_json(_to_json(item), blocking=True)
        core.end()
        return cls(core, name=name)


class Context:
    def __init__(self, core: _ContextCore) -> None:
        self._core = core

    @classmethod
    async def create(cls, threads: int = 0) -> Context:
        core = await asyncio.to_thread(_ContextCore, threads)
        return cls(core)

    async def configure(
        self,
        *,
        cache_dir: str | object | None = _UNSET,
        max_memory: int | object = _UNSET,
        prelude: str | object | None = _UNSET,
        runtime_lib_dir: str | object | None = _UNSET,
        mounts: list[MountConfig] | object | None = _UNSET,
        env: dict[str, str] | object = _UNSET,
    ) -> None:
        patch: dict[str, object] = {}

        if cache_dir is not _UNSET:
            patch["cache_dir"] = cache_dir
        if max_memory is not _UNSET:
            patch["max_memory"] = max_memory
        if prelude is not _UNSET:
            patch["prelude"] = prelude
        if runtime_lib_dir is not _UNSET:
            patch["runtime_lib_dir"] = runtime_lib_dir
        if mounts is not _UNSET:
            if mounts is None:
                patch["mounts"] = None
            elif isinstance(mounts, list):
                mounts_list = cast("list[MountConfig]", mounts)
                patch["mounts"] = [mount.to_dict() for mount in mounts_list]
            else:
                msg = "mounts must be a list[MountConfig] or None"
                raise TypeError(msg)
        if env is not _UNSET:
            patch["env"] = env

        if patch:
            payload = json.dumps(patch)
            await asyncio.to_thread(self._core.configure_json, payload)

    async def initialize_template(self, runtime_path: str) -> None:
        await asyncio.to_thread(self._core.initialize_template, runtime_path)

    async def instantiate(self, *, config: SandboxConfig | None = None) -> Sandbox:
        core = await asyncio.to_thread(self._core.instantiate)
        sandbox = Sandbox(core)
        if config is not None:
            await sandbox.configure(**config.to_patch())
        return sandbox

    async def close(self) -> None:
        await asyncio.to_thread(self._core.close)


class Sandbox:
    def __init__(self, core: _SandboxCore) -> None:
        self._core = core
        self._callback: Callable[[Event], Awaitable[None] | None] | None = None
        self._dispatch: Callable[[str, str | None], None] | None = None
        self._http_handler: (
            Callable[[HttpRequestData], Awaitable[HttpResponseData]] | None
        ) = None
        self._http_dispatch: (
            Callable[
                [str, str, dict[str, str], bytes | None],
                Awaitable[tuple[int, dict[str, str], str, object]],
            ]
            | None
        ) = None
        self._pending_callback_tasks: set[asyncio.Task[None]] = set()

    async def configure(
        self,
        *,
        max_memory: int | object | None = _UNSET,
        timeout_ms: int | object | None = _UNSET,
        mounts: list[MountConfig] | object | None = _UNSET,
        env: dict[str, str] | object = _UNSET,
    ) -> None:
        patch: dict[str, object] = {}

        if max_memory is not _UNSET:
            patch["max_memory"] = max_memory
        if timeout_ms is not _UNSET:
            patch["timeout_ms"] = timeout_ms
        if mounts is not _UNSET:
            if mounts is None:
                patch["mounts"] = None
            elif isinstance(mounts, list):
                mounts_list = cast("list[MountConfig]", mounts)
                patch["mounts"] = [mount.to_dict() for mount in mounts_list]
            else:
                msg = "mounts must be a list[MountConfig] or None"
                raise TypeError(msg)
        if env is not _UNSET:
            patch["env"] = env

        if patch:
            payload = json.dumps(patch)
            await asyncio.to_thread(self._core.configure_json, payload)

    def set_callback(
        self, callback: Callable[[Event], Awaitable[None] | None] | None
    ) -> None:
        self._callback = callback

        if callback is None:
            self._dispatch = None
            self._core.set_callback(None)
            return

        loop = asyncio.get_running_loop()

        def _dispatch(kind: str, data: str | None) -> None:
            event = Event(kind=cast("EventKind", kind), data=data)

            def _invoke() -> None:
                outcome = callback(event)
                if inspect.isawaitable(outcome):
                    future = asyncio.ensure_future(outcome)
                    self._pending_callback_tasks.add(future)
                    future.add_done_callback(self._pending_callback_tasks.discard)

            loop.call_soon_threadsafe(_invoke)

        self._dispatch = _dispatch
        self._core.set_callback(_dispatch)

    def set_http_handler(
        self, handler: Callable[[HttpRequestData], Awaitable[HttpResponseData]] | None
    ) -> None:
        self._http_handler = handler
        if handler is None:
            self._http_dispatch = None
            self._core.set_http_handler(None, None)
            return

        loop = asyncio.get_running_loop()

        async def _dispatch(
            method: str, url: str, headers: dict[str, str], body: bytes | None
        ) -> tuple[int, dict[str, str], str, object]:
            request = HttpRequestData(
                method=method, url=url, headers=dict(headers), body=body
            )
            response_obj = cast("object", await handler(request))
            if not isinstance(response_obj, HttpResponseData):
                msg = "http handler must return HttpResponseData"
                raise TypeError(msg)
            response = response_obj
            body_mode, body_payload = _normalize_http_response_body(response.body)
            return (response.status, dict(response.headers), body_mode, body_payload)

        self._http_dispatch = _dispatch
        self._core.set_http_handler(_dispatch, loop)

    async def start(self) -> None:
        await asyncio.to_thread(self._core.start)

    async def load_script(self, code: str, timeout_s: float | None = None) -> None:
        operation = asyncio.to_thread(self._core.load_script, code, None)
        await _await_with_timeout(operation, timeout_s)

    async def run(
        self,
        func: str,
        args: list[JsonArg | JsonStreamArg | object] | None = None,
        timeout_s: float | None = None,
    ) -> RunResult:
        encoded_args, producers = _encode_args(args)
        operation = asyncio.to_thread(self._core.run, func, encoded_args, None)

        try:
            core_result = await _await_with_timeout(operation, timeout_s)
        except Exception:
            for producer in producers:
                producer.cancel()
            if producers:
                await asyncio.gather(*producers, return_exceptions=True)
            raise

        if producers:
            await asyncio.gather(*producers)

        return _convert_result(core_result)

    async def close(self) -> None:
        self._http_handler = None
        self._http_dispatch = None
        await asyncio.to_thread(self._core.close)


class RuntimeDownloadError(IsolaError):
    pass


class RuntimeManager:
    @staticmethod
    async def ensure_latest(
        *, cache_subdir: str = "isola/runtime", force: bool = False
    ) -> str:
        operation = partial(_ensure_latest_runtime_sync, cache_subdir, force=force)
        return await asyncio.to_thread(operation)


def _ensure_latest_runtime_sync(cache_subdir: str, *, force: bool) -> str:
    runtime_root = _data_root() / cache_subdir
    wasm_path = runtime_root / "bin" / "python3.wasm"

    if wasm_path.exists() and not force:
        return str(runtime_root)

    runtime_root.mkdir(parents=True, exist_ok=True)
    _clear_directory(runtime_root)

    with tempfile.NamedTemporaryFile(suffix=".tar.gz", delete=True) as archive:
        try:
            with urllib.request.urlopen(_RUNTIME_BUNDLE_URL, timeout=120) as response:
                archive.write(response.read())
                archive.flush()
        except Exception as exc:
            message = f"failed to download runtime bundle: {exc}"
            raise RuntimeDownloadError(message) from exc

        try:
            with tarfile.open(archive.name, mode="r:gz") as tar:
                _safe_extract(tar, runtime_root)
        except Exception as exc:
            message = f"failed to extract runtime bundle: {exc}"
            raise RuntimeDownloadError(message) from exc

    _normalize_runtime_layout(runtime_root)

    if not wasm_path.exists():
        message = f"runtime bundle is missing '{wasm_path}' after extraction"
        raise RuntimeDownloadError(message)

    return str(runtime_root)


def _safe_extract(tar: tarfile.TarFile, target_dir: Path) -> None:
    target_dir = target_dir.resolve()

    for member in tar.getmembers():
        destination = (target_dir / member.name).resolve()
        if destination != target_dir and target_dir not in destination.parents:
            message = "runtime archive contains unsafe paths"
            raise RuntimeDownloadError(message)

    for member in tar.getmembers():
        tar.extract(member, target_dir)


def _normalize_runtime_layout(runtime_root: Path) -> None:
    wasm_path = runtime_root / "bin" / "python3.wasm"
    if wasm_path.exists():
        return

    candidates = [entry for entry in runtime_root.iterdir() if entry.is_dir()]
    if len(candidates) != 1:
        return

    nested_root = candidates[0]
    nested_wasm = nested_root / "bin" / "python3.wasm"
    if not nested_wasm.exists():
        return

    for item in nested_root.iterdir():
        shutil.move(str(item), runtime_root / item.name)
    nested_root.rmdir()


def _clear_directory(directory: Path) -> None:
    for item in directory.iterdir():
        if item.is_dir():
            shutil.rmtree(item)
        else:
            item.unlink()


def _data_root() -> Path:
    if os.name == "nt":
        appdata = os.environ.get("APPDATA")
        if appdata:
            return Path(appdata)
        return Path.home() / "AppData" / "Roaming"

    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support"

    xdg = os.environ.get("XDG_DATA_HOME")
    if xdg:
        return Path(xdg)
    return Path.home() / ".local" / "share"


def _to_json(value: object) -> str:
    return json.dumps(value, separators=(",", ":"))


def _encode_args(
    args: list[JsonArg | JsonStreamArg | object] | None,
) -> tuple[list[tuple[str, str | None, object]], list[asyncio.Task[None]]]:
    if args is None:
        return [], []

    encoded: list[tuple[str, str | None, object]] = []
    producers: list[asyncio.Task[None]] = []

    for arg in args:
        if isinstance(arg, JsonArg):
            encoded.append(("json", arg.name, _to_json(arg.value)))
            continue

        if isinstance(arg, JsonStreamArg):
            encoded.append(("stream", arg.name, arg.stream_core))
            if arg.producer_task is not None:
                producers.append(arg.producer_task)
            continue

        encoded.append(("json", None, _to_json(arg)))

    return encoded, producers


def _convert_result(core_result: _RunResultCore) -> RunResult:
    results = [cast("JsonValue", json.loads(item)) for item in core_result.result_json]
    if core_result.final_json is None:
        final = None
    else:
        final = cast("JsonValue", json.loads(core_result.final_json))

    return RunResult(
        results=results,
        final=final,
        stdout=list(core_result.stdout),
        stderr=list(core_result.stderr),
        logs=list(core_result.logs),
        errors=list(core_result.errors),
    )


def _normalize_http_response_body(body: HttpBodyOut) -> tuple[str, object]:
    if body is None:
        return ("none", None)

    if isinstance(body, (bytes, bytearray, memoryview)):
        return ("bytes", bytes(body))

    if not hasattr(body, "__aiter__"):
        msg = "http response body must be bytes, AsyncIterable[bytes], or None"
        raise TypeError(msg)

    async def _stream_body(source: AsyncIterable[bytes]) -> AsyncIterable[bytes]:
        async for chunk in source:
            yield bytes(chunk)

    return ("stream", _stream_body(body))


async def _await_with_timeout(awaitable: Awaitable[T], timeout_s: float | None) -> T:
    if timeout_s is None:
        return await awaitable

    timeout_ctx = getattr(asyncio, "timeout", None)
    if timeout_ctx is not None:
        timeout_factory = cast(
            "Callable[[float], AbstractAsyncContextManager[None]]", timeout_ctx
        )
        async with timeout_factory(timeout_s):
            return await awaitable

    return await asyncio.wait_for(awaitable, timeout_s)
