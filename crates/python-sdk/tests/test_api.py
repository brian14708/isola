from __future__ import annotations

import asyncio
import os
from collections.abc import Awaitable, Callable
from pathlib import Path
from typing import TYPE_CHECKING, NoReturn, cast

import pytest

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    from isola import HttpRequestData, HttpResponseData

isola = pytest.importorskip("isola")

_HttpHandlerDispatch = Callable[
    [str, str, dict[str, str], bytes | None],
    Awaitable[tuple[int, dict[str, str], str, object]],
]


class _FakeSandboxCore:
    def __init__(self) -> None:
        self.config_json: str | None = None
        self.callback: Callable[[str, str | None], None] | None = None
        self.http_handler: _HttpHandlerDispatch | None = None
        self.http_loop: asyncio.AbstractEventLoop | None = None
        self.started = False
        self.loaded_script: tuple[str, int | None] | None = None
        self.last_run: (
            tuple[str, list[tuple[str, str | None, object]] | None, int | None] | None
        ) = None
        self.closed = False

    def configure_json(self, config_json: str) -> None:
        self.config_json = config_json

    def set_callback(self, callback: Callable[[str, str | None], None] | None) -> None:
        self.callback = callback

    def set_http_handler(
        self,
        callback: _HttpHandlerDispatch | None,
        event_loop: asyncio.AbstractEventLoop | None,
    ) -> None:
        self.http_handler = callback
        self.http_loop = event_loop

    def start(self) -> None:
        self.started = True

    def load_script(self, code: str, timeout_ms: int | None = None) -> None:
        self.loaded_script = (code, timeout_ms)

    def run(
        self,
        func: str,
        args: list[tuple[str, str | None, object]] | None = None,
        timeout_ms: int | None = None,
    ) -> NoReturn:
        self.last_run = (func, args, timeout_ms)
        raise AssertionError

    def close(self) -> None:
        self.closed = True


def test_json_stream_from_iterable_roundtrip() -> None:
    stream_arg = isola.JsonStreamArg.from_iterable([1, 2, 3])
    assert stream_arg.name is None


def _resolve_runtime_paths() -> tuple[Path, Path]:
    workspace_root = Path(__file__).resolve().parents[3]
    runtime_dir = workspace_root / "target"
    wasm_path = runtime_dir / "python3.wasm"
    if not wasm_path.is_file():
        message = (
            f"missing runtime wasm at '{wasm_path}', build with `cargo xtask build-all`"
        )
        pytest.skip(message)

    wasi_runtime = os.environ.get("WASI_PYTHON_RUNTIME")
    if wasi_runtime is None:
        lib_dir = runtime_dir / "wasm32-wasip1" / "wasi-deps" / "usr" / "local" / "lib"
    else:
        lib_dir = Path(wasi_runtime) / "lib"

    if not lib_dir.is_dir():
        message = (
            f"missing runtime libs at '{lib_dir}', "
            "set WASI_PYTHON_RUNTIME or build runtime deps"
        )
        pytest.skip(message)

    return runtime_dir, lib_dir


@pytest.mark.asyncio
async def test_context_creation_smoke() -> None:
    ctx = await isola.Context.create(threads=0)
    await ctx.close()


@pytest.mark.asyncio
async def test_asyncio_timeout_wrapper() -> None:
    event = asyncio.Event()

    with pytest.raises(TimeoutError):
        await asyncio.wait_for(event.wait(), timeout=0.001)


@pytest.mark.asyncio
async def test_can_start_sandbox_and_execute_code() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    context = await isola.Context.create(threads=0)
    try:
        await context.configure(
            max_memory=64 * 1024 * 1024, runtime_lib_dir=str(lib_dir)
        )
        await context.initialize_template(str(runtime_dir))

        sandbox = await context.instantiate(
            config=isola.SandboxConfig(timeout_ms=1_000)
        )
        try:
            await sandbox.start()
            await sandbox.load_script(
                "def add(a, b):\n"
                "\treturn a + b\n"
                "\n"
                "def stream_values(n):\n"
                "\tfor i in range(n):\n"
                "\t\tyield i"
            )

            add_result = await sandbox.run("add", [1, 2])
            assert add_result.results == []
            assert add_result.final == 3

            stream_result = await sandbox.run("stream_values", [3])
            assert stream_result.results == [0, 1, 2]
            assert stream_result.final is None
        finally:
            await sandbox.close()
    finally:
        await context.close()


@pytest.mark.asyncio
async def test_sandbox_http_handler_bytes_response_shape() -> None:
    core = _FakeSandboxCore()
    sandbox = isola.Sandbox(core)

    async def handler(_: HttpRequestData) -> HttpResponseData:
        await asyncio.sleep(0)
        return cast(
            "HttpResponseData",
            isola.HttpResponseData(
                status=201, headers={"content-type": "text/plain"}, body=b"ok"
            ),
        )

    sandbox.set_http_handler(handler)
    http_handler = core.http_handler
    assert http_handler is not None
    assert core.http_loop is not None

    status, headers, mode, payload = await http_handler(
        "GET", "https://example.com", {"x-test": "1"}, None
    )
    assert status == 201
    assert headers == {"content-type": "text/plain"}
    assert mode == "bytes"
    assert payload == b"ok"


@pytest.mark.asyncio
async def test_sandbox_http_handler_stream_response_shape() -> None:
    core = _FakeSandboxCore()
    sandbox = isola.Sandbox(core)

    async def body() -> AsyncIterator[bytes]:
        await asyncio.sleep(0)
        yield b"a"
        await asyncio.sleep(0)
        yield b"b"

    async def handler(_: HttpRequestData) -> HttpResponseData:
        await asyncio.sleep(0)
        return cast("HttpResponseData", isola.HttpResponseData(status=200, body=body()))

    sandbox.set_http_handler(handler)
    http_handler = core.http_handler
    assert http_handler is not None
    _, _, mode, payload = await http_handler("GET", "https://example.com", {}, None)
    assert mode == "stream"
    stream = cast("AsyncIterator[bytes]", payload)
    chunks = [item async for item in stream]
    assert chunks == [b"a", b"b"]


@pytest.mark.asyncio
async def test_real_sandbox_http_fetch_uses_python_handler_stream() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    context = await isola.Context.create(threads=0)
    try:
        await context.configure(
            max_memory=64 * 1024 * 1024, runtime_lib_dir=str(lib_dir)
        )
        await context.initialize_template(str(runtime_dir))

        sandbox = await context.instantiate(
            config=isola.SandboxConfig(timeout_ms=1_000)
        )
        try:

            async def response_chunks() -> AsyncIterator[bytes]:
                await asyncio.sleep(0)
                yield b"hello "
                await asyncio.sleep(0)
                yield b"world"

            async def http_handler(req: HttpRequestData) -> HttpResponseData:
                assert req.method == "GET"
                assert req.url == "https://example.test/stream"
                await asyncio.sleep(0)
                return cast(
                    "HttpResponseData",
                    isola.HttpResponseData(
                        status=200,
                        headers={"content-type": "text/plain", "x-test": "stream"},
                        body=response_chunks(),
                    ),
                )

            sandbox.set_http_handler(http_handler)
            await sandbox.start()
            await sandbox.load_script(
                "from sandbox.http import fetch\n"
                "\n"
                "def main(url):\n"
                "\twith fetch('GET', url) as resp:\n"
                "\t\tdata = b''.join(resp.iter_bytes())\n"
                "\t\treturn [resp.status, resp.headers.get('x-test'), data.decode()]\n"
            )

            result = await sandbox.run("main", ["https://example.test/stream"])
            assert result.results == []
            assert result.final == [200, "stream", "hello world"]
            assert result.errors == []
        finally:
            await sandbox.close()
    finally:
        await context.close()
