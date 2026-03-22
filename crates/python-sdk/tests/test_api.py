from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

import pytest

if TYPE_CHECKING:
    from collections.abc import AsyncIterator, Awaitable, Callable

    from isola import HttpRequest, HttpResponse
    from isola import Sandbox as IsolaSandbox

isola = pytest.importorskip("isola")
runtime_module = pytest.importorskip("isola._runtime")

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


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("runtime", "bundle_file", "lib_subdir"),
    [("python", "python.wasm", "lib"), ("js", "js.wasm", None)],
)
async def test_resolve_runtime_uses_flat_cache_layout(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    runtime: str,
    bundle_file: str,
    lib_subdir: str | None,
) -> None:
    cache_base = tmp_path / "cache"
    runtime_root = cache_base / "isola" / "runtimes" / f"{runtime}-0.2.0"
    (runtime_root / "bin").mkdir(parents=True)
    (runtime_root / "bin" / bundle_file).write_bytes(b"")
    if lib_subdir is not None:
        (runtime_root / lib_subdir).mkdir()

    monkeypatch.setenv("XDG_CACHE_HOME", str(cache_base))

    config = await isola.resolve_runtime(cast("Any", runtime), version="0.2.0")
    assert config["runtime_path"] == runtime_root / "bin"
    if lib_subdir is not None:
        assert config["runtime_lib_dir"] == runtime_root / lib_subdir


def test_strip_first_path_component_flattens_bundle_root() -> None:
    strip_first_path_component = runtime_module._strip_first_path_component  # noqa: SLF001

    assert (
        strip_first_path_component("isola-python-runtime/bin/python.wasm")
        == "bin/python.wasm"
    )
    assert strip_first_path_component("isola-python-runtime") is None


def _resolve_runtime_paths() -> tuple[Path, Path]:
    workspace_root = Path(__file__).resolve().parents[3]
    runtime_dir = workspace_root / "target"
    wasm_path = runtime_dir / "python.wasm"
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


def _resolve_js_runtime_dir() -> Path:
    workspace_root = Path(__file__).resolve().parents[3]
    runtime_dir = workspace_root / "target"
    wasm_path = runtime_dir / "js.wasm"
    if not wasm_path.is_file():
        message = (
            f"missing JS runtime wasm at '{wasm_path}', "
            "build with `cargo xtask build-js`"
        )
        pytest.skip(message)
    return runtime_dir


def test_context_creation_smoke() -> None:
    with isola.SandboxManager():
        pass


def test_sandbox_config_defaults_are_unlimited() -> None:
    config: dict[str, object] = {}
    assert config.get("max_memory") is None
    assert "timeout" not in config


@pytest.mark.asyncio
async def test_async_context_managers_smoke() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    sandbox = await template.create()
    async with sandbox:
        await sandbox.load_script("def ping():\n\treturn 'ok'")
        result = await sandbox.run("ping")
        assert result.results == []
        assert result.final == "ok"
    mgr.close()


@pytest.mark.asyncio
async def test_async_context_managers_js_runtime_smoke() -> None:
    runtime_dir = _resolve_js_runtime_dir()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template("js", runtime_path=runtime_dir)

    sandbox = await template.create()
    async with sandbox:
        await sandbox.load_script("function ping() { return 'ok'; }")
        result = await sandbox.run("ping")
        assert result.results == []
        assert result.final == "ok"
    mgr.close()


@pytest.mark.asyncio
async def test_initialize_template_rejects_unknown_runtime() -> None:
    mgr = isola.SandboxManager()
    with pytest.raises(isola.InvalidArgumentError, match="unsupported runtime"):
        await mgr.compile_template(cast("str", "ruby"), runtime_path=".")
    mgr.close()


@pytest.mark.asyncio
async def test_asyncio_timeout_wrapper() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    sandbox = await template.create()
    async with sandbox:
        await sandbox.load_script(
            "import time\n\ndef slow():\n\ttime.sleep(0.2)\n\treturn 1\n"
        )

        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(sandbox.run("slow"), timeout=0.001)
    mgr.close()


@pytest.mark.asyncio
async def test_can_start_sandbox_and_execute_code() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    sandbox = await template.create()
    async with sandbox:
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
    mgr.close()


@pytest.mark.asyncio
async def test_run_stream_yields_events() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    sandbox = await template.create()
    async with sandbox:
        await sandbox.load_script("def emit():\n\tprint('hello')\n\treturn 7\n")

        events = [event async for event in sandbox.run_stream("emit")]
        assert events
        assert any(event.kind == "stdout" for event in events)
        end_events = [event for event in events if event.kind == "end"]
        assert len(end_events) == 1
        assert end_events[0].data == "7"
    mgr.close()


@pytest.mark.asyncio
async def test_template_create_hostcalls_json_roundtrip() -> None:
    class _FakeCore:
        def __init__(self) -> None:
            self.callback: Callable[[str, str | None], None] | None = None
            self.hostcall_handler: (
                Callable[[str, str], Awaitable[str]] | None
            ) = None
            self.hostcall_loop: asyncio.AbstractEventLoop | None = None
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

        def set_hostcall_handler(
            self,
            callback: Callable[[str, str], Awaitable[str]] | None,
            event_loop: asyncio.AbstractEventLoop | None,
        ) -> None:
            self.hostcall_handler = callback
            self.hostcall_loop = event_loop

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

        def close(self) -> None:
            pass

    class _FakeContextCore:
        def __init__(self, sandbox_core: _FakeCore) -> None:
            self.sandbox_core = sandbox_core

        async def instantiate(self) -> _FakeCore:
            return self.sandbox_core

    core = _FakeCore()
    template = isola.SandboxTemplate(cast("Any", _FakeContextCore(core)))

    async def lookup_user(payload: object) -> object:
        assert payload == {"user_id": 7}
        await asyncio.sleep(0)
        return {"id": 7, "name": "user-7"}

    await template.create(hostcalls={"lookup_user": lookup_user})

    callback = core.hostcall_handler
    assert callback is not None
    assert core.hostcall_loop is asyncio.get_running_loop()
    assert await callback("lookup_user", '{"user_id":7}') == '{"id":7,"name":"user-7"}'
    with pytest.raises(ValueError, match="unsupported hostcall: missing"):
        await callback("missing", "{}")

    empty_core = _FakeCore()
    empty_template = isola.SandboxTemplate(cast("Any", _FakeContextCore(empty_core)))
    await empty_template.create()
    assert empty_core.hostcall_handler is None
    assert empty_core.hostcall_loop is None


@pytest.mark.asyncio
async def test_run_stream_flushes_trailing_scheduled_events() -> None:
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
            callback("stdout", "hello")
            callback("end_json", "7")

        def close(self) -> None:
            pass

    core = _FakeCore()
    sandbox = isola.Sandbox(cast("Any", core))

    events = [event async for event in sandbox.run_stream("emit")]
    assert [(event.kind, event.data) for event in events] == [
        ("stdout", "hello"),
        ("end", "7"),
    ]


@pytest.mark.asyncio
async def test_invalid_later_arg_does_not_start_stream_producer() -> None:
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
            _ = self
            _ = func
            _ = args
            message = "sandbox.run() should not be called when argument encoding fails"
            raise AssertionError(message)

        def close(self) -> None:
            pass

    started = asyncio.Event()

    async def values() -> AsyncIterator[int]:
        started.set()
        await asyncio.sleep(0)
        yield 1

    stream_arg = isola.StreamArg.from_async_iterable(values())
    sandbox = isola.Sandbox(cast("Any", _FakeCore()))

    with pytest.raises(TypeError):
        await sandbox.run("emit", [stream_arg, object()])

    await asyncio.sleep(0)
    assert not started.is_set()
    assert stream_arg.producer_task is None


@pytest.mark.asyncio
async def test_two_sandboxes_can_run_concurrently() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    sandbox_a = await template.create()
    sandbox_b = await template.create()
    async with sandbox_a, sandbox_b:
        script = (
            "import time\n"
            "\n"
            "def identify(name, delay):\n"
            "\ttime.sleep(delay)\n"
            "\treturn name\n"
        )

        async def _run_one(sandbox: IsolaSandbox, name: str) -> str | None:
            await sandbox.load_script(script)
            result = await sandbox.run("identify", [name, 0.05])
            return cast("str | None", result.final)

        result_a, result_b = await asyncio.gather(
            _run_one(sandbox_a, "sandbox-a"), _run_one(sandbox_b, "sandbox-b")
        )
        assert result_a == "sandbox-a"
        assert result_b == "sandbox-b"
    mgr.close()


@pytest.mark.asyncio
async def test_sandbox_http_handler_bytes_response_shape() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

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

    sandbox = await template.create(http_handler=http_handler)
    async with sandbox:
        await sandbox.load_script(_FETCH_SCRIPT)

        result = await sandbox.run("main", ["https://example.test/bytes"])
        assert result.results == []
        assert result.final == [201, "bytes", "ok"]
        assert result.errors == []
    mgr.close()


@pytest.mark.asyncio
async def test_sandbox_hostcalls_roundtrip() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    async def lookup_user(payload: object) -> object:
        assert payload == {"user_id": 7}
        await asyncio.sleep(0)
        return {"user_id": 7, "name": "user-7"}

    sandbox = await template.create(hostcalls={"lookup_user": lookup_user})
    async with sandbox:
        await sandbox.load_script(
            "from sandbox.asyncio import hostcall\n"
            "\n"
            "async def main(user_id):\n"
            "\treturn await hostcall('lookup_user', {'user_id': user_id})\n"
        )

        result = await sandbox.run("main", [7])
        assert result.results == []
        assert result.final == {"user_id": 7, "name": "user-7"}
        assert result.errors == []
    mgr.close()


@pytest.mark.asyncio
async def test_sandbox_http_handler_stream_response_shape() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

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

    sandbox = await template.create(http_handler=http_handler)
    async with sandbox:
        await sandbox.load_script(_FETCH_SCRIPT)

        result = await sandbox.run("main", ["https://example.test/stream"])
        assert result.results == []
        assert result.final == [200, "stream", "ab"]
        assert result.errors == []
    mgr.close()


@pytest.mark.asyncio
async def test_sandbox_caller_timeout_does_not_break_following_requests() -> None:
    runtime_dir, lib_dir = _resolve_runtime_paths()
    mgr = isola.SandboxManager()
    template = await mgr.compile_template(
        "python",
        runtime_path=runtime_dir,
        max_memory=64 * 1024 * 1024,
        runtime_lib_dir=lib_dir,
    )

    async def warmup_handler(_: HttpRequest) -> HttpResponse:
        await asyncio.sleep(0)
        return cast("HttpResponse", isola.HttpResponse(status=200, body=b"ok"))

    warmup = await template.create(http_handler=warmup_handler)
    async with warmup:
        await warmup.load_script(_FETCH_SCRIPT)
        result = await warmup.run("main", ["https://example.test/warmup"])
        assert result.final == [200, None, "ok"]

    for _ in range(4):

        async def hanging_handler(_: HttpRequest) -> HttpResponse:
            await asyncio.Event().wait()
            message = "unreachable"
            raise AssertionError(message)

        sandbox = await template.create(http_handler=hanging_handler)
        async with sandbox:
            await sandbox.load_script(_FETCH_SCRIPT)
            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(
                    sandbox.run("main", ["https://example.test/hang"]), timeout=0.05
                )

        await asyncio.sleep(0.05)

    async def recovery_handler(_: HttpRequest) -> HttpResponse:
        await asyncio.sleep(0)
        return cast("HttpResponse", isola.HttpResponse(status=200, body=b"ok"))

    sandbox = await template.create(http_handler=recovery_handler)
    async with sandbox:
        await sandbox.load_script(_FETCH_SCRIPT)
        result = await sandbox.run("main", ["https://example.test/recovery"])
        assert result.final == [200, None, "ok"]
    mgr.close()
