from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

import pytest

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable

    from isola import Event, HttpRequest, HttpResponse
    from isola import Sandbox as IsolaSandbox

isola = pytest.importorskip("isola")

_FETCH_SCRIPT = (
    "from sandbox.http import fetch\n"
    "\n"
    "def main(url):\n"
    "\twith fetch('GET', url) as resp:\n"
    "\t\tdata = b''.join(resp.iter_bytes())\n"
    "\t\treturn [resp.status, resp.headers.get('x-test'), data.decode()]\n"
)


def test_json_stream_from_iterable_roundtrip() -> None:
    stream_arg = isola.StreamArg.from_iterable([1, 2], capacity=1)
    assert stream_arg.name is None
    assert stream_arg.producer_task is None


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


def test_context_creation_smoke() -> None:
    with isola.Context.create():
        pass


def test_sandbox_config_defaults_are_unlimited() -> None:
    patch = isola.SandboxConfig().to_patch()
    assert patch["max_memory"] is None
    assert patch["timeout"] is None


@pytest.mark.asyncio
async def test_async_context_managers_smoke() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)

        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=1.0)
        ) as sandbox:
            await sandbox.start()
            await sandbox.load_script("def ping():\n\treturn 'ok'")
            result = await sandbox.run("ping")
            assert result.results == []
            assert result.final == "ok"


@pytest.mark.asyncio
async def test_asyncio_timeout_wrapper() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=30.0)
        ) as sandbox:
            await sandbox.start()
            await sandbox.load_script(
                "import time\n\ndef slow():\n\ttime.sleep(0.2)\n\treturn 1\n"
            )

            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(sandbox.run("slow"), timeout=0.001)


@pytest.mark.asyncio
async def test_can_start_sandbox_and_execute_code() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=1.0)
        ) as sandbox:
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


@pytest.mark.asyncio
async def test_run_stream_yields_events() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=1.0)
        ) as sandbox:
            await sandbox.start()
            await sandbox.load_script("def emit():\n\tprint('hello')\n\treturn 7\n")

            events = [event async for event in sandbox.run_stream("emit")]
            assert events
            assert any(event.kind == "stdout" for event in events)
            end_events = [event for event in events if event.kind == "end"]
            assert len(end_events) == 1
            assert end_events[0].data == "7"


@pytest.mark.asyncio
async def test_set_callback_during_run_stream_keeps_latest_callback() -> None:
    class _FakeCore:
        def __init__(self) -> None:
            self.callback: Callable[[str, str | None], None] | None = None
            self.http_handler: (
                Callable[
                    [str, str, dict[str, str], bytes | None],
                    Awaitable[tuple[int, dict[str, str], str, object]],
                ]
                | None
            ) = None
            self.http_loop: asyncio.AbstractEventLoop | None = None

        def set_callback(
            self, callback: Callable[[str, str | None], None] | None
        ) -> None:
            self.callback = callback

        def set_http_handler(
            self,
            callback: Callable[
                [str, str, dict[str, str], bytes | None],
                Awaitable[tuple[int, dict[str, str], str, object]],
            ]
            | None,
            event_loop: asyncio.AbstractEventLoop | None,
        ) -> None:
            self.http_handler = callback
            self.http_loop = event_loop

        async def run(
            self, func: str, args: list[tuple[str, str | None, object]]
        ) -> None:
            _ = func
            _ = args
            callback = self.callback
            assert callback is not None
            callback("stdout", "first")
            await asyncio.sleep(0)
            callback("stdout", "second")
            callback("end_json", "1")

        def close(self) -> None:
            pass

    core = _FakeCore()
    sandbox = isola.Sandbox(cast("Any", core))

    seen_a: list[tuple[str, str | None]] = []
    seen_b: list[tuple[str, str | None]] = []

    def callback_a(event: Event) -> None:
        seen_a.append((event.kind, event.data))

    def callback_b(event: Event) -> None:
        seen_b.append((event.kind, event.data))

    sandbox.set_callback(callback_a)

    async for event in sandbox.run_stream("emit"):
        if event.kind == "stdout" and event.data == "first":
            sandbox.set_callback(callback_b)

    callback = core.callback
    assert callback is not None
    callback("stdout", "after")
    await asyncio.sleep(0)

    assert ("stdout", "after") not in seen_a
    assert ("stdout", "after") in seen_b


@pytest.mark.asyncio
async def test_two_sandboxes_can_run_concurrently() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with (
            await context.instantiate(
                config=isola.SandboxConfig(timeout=2.0)
            ) as sandbox_a,
            await context.instantiate(
                config=isola.SandboxConfig(timeout=2.0)
            ) as sandbox_b,
        ):
            script = (
                "import time\n"
                "\n"
                "def identify(name, delay):\n"
                "\ttime.sleep(delay)\n"
                "\treturn name\n"
            )

            async def _run_one(sandbox: IsolaSandbox, name: str) -> str | None:
                await sandbox.start()
                await sandbox.load_script(script)
                result = await sandbox.run("identify", [name, 0.05])
                return cast("str | None", result.final)

            result_a, result_b = await asyncio.gather(
                _run_one(sandbox_a, "sandbox-a"), _run_one(sandbox_b, "sandbox-b")
            )
            assert result_a == "sandbox-a"
            assert result_b == "sandbox-b"


@pytest.mark.asyncio
async def test_sandbox_http_handler_bytes_response_shape() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=1.0)
        ) as sandbox:

            async def http_handler(_: HttpRequest) -> HttpResponse:
                await asyncio.sleep(0)
                return cast(
                    "HttpResponse",
                    isola.HttpResponse(
                        status=201,
                        headers={"content-type": "text/plain", "x-test": "bytes"},
                        body=b"ok",
                    ),
                )

            sandbox.set_http_handler(http_handler)
            await sandbox.start()
            await sandbox.load_script(_FETCH_SCRIPT)

            result = await sandbox.run("main", ["https://example.test/bytes"])
            assert result.results == []
            assert result.final == [201, "bytes", "ok"]
            assert result.errors == []


@pytest.mark.asyncio
async def test_sandbox_http_handler_stream_response_shape() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    async with isola.Context.create() as context:
        context.configure(max_memory=64 * 1024 * 1024, runtime_lib_dir=lib_dir)
        await context.initialize_template(runtime_dir)
        async with await context.instantiate(
            config=isola.SandboxConfig(timeout=1.0)
        ) as sandbox:

            async def response_chunks() -> AsyncIterator[bytes]:
                await asyncio.sleep(0)
                yield b"a"
                await asyncio.sleep(0)
                yield b"b"

            async def http_handler(req: HttpRequest) -> HttpResponse:
                assert req.method == "GET"
                assert req.url == "https://example.test/stream"
                await asyncio.sleep(0)
                return cast(
                    "HttpResponse",
                    isola.HttpResponse(
                        status=200,
                        headers={"content-type": "text/plain", "x-test": "stream"},
                        body=response_chunks(),
                    ),
                )

            sandbox.set_http_handler(http_handler)
            await sandbox.start()
            await sandbox.load_script(_FETCH_SCRIPT)

            result = await sandbox.run("main", ["https://example.test/stream"])
            assert result.results == []
            assert result.final == [200, "stream", "ab"]
            assert result.errors == []
