"""Managed isola-server subprocess for e2e tests."""

from __future__ import annotations

import os
import shutil
import subprocess  # noqa: S404
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO

import httpx

_PROJECT_ROOT = Path(__file__).resolve().parents[4]
_DEFAULT_PORT = 3001


class ServerStartError(RuntimeError):
    """Raised when the server fails to start or become healthy."""

    def __init__(self, summary: str, log_tail: str) -> None:
        super().__init__(f"{summary}\n{log_tail}")


@dataclass
class ServerProcess:
    """A running isola-server instance."""

    base_url: str
    process: subprocess.Popen[bytes] | None = None
    log_file: IO[bytes] | None = field(default=None, repr=False)

    def stop(self) -> None:
        if self.process is not None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        if self.log_file is not None:
            self.log_file.close()


def _find_cargo() -> str:
    path = shutil.which("cargo")
    if path is None:
        msg = "cargo not found on PATH"
        raise FileNotFoundError(msg)
    return path


def start_server(
    *,
    port: int = _DEFAULT_PORT,
    timeout: float = 60.0,
    poll_interval: float = 0.5,
) -> ServerProcess:
    """Build and start isola-server, blocking until healthy.

    Raises ServerStartError if the server exits or fails to become healthy
    within *timeout* seconds.
    """
    log_dir = _PROJECT_ROOT / "target"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / "e2e-server.log"
    log_file = log_path.open("wb")

    env = {**os.environ, "PORT": str(port)}
    cargo = _find_cargo()
    proc = subprocess.Popen(  # noqa: S603
        [cargo, "run", "--release", "-p", "isola-server"],
        cwd=_PROJECT_ROOT,
        stdout=log_file,
        stderr=subprocess.STDOUT,
        env=env,
    )

    base_url = f"http://127.0.0.1:{port}"
    health_url = f"{base_url}/debug/healthz"
    deadline = time.monotonic() + timeout

    while time.monotonic() < deadline:
        ret = proc.poll()
        if ret is not None:
            log_file.close()
            msg = f"isola-server exited with code {ret} during startup"
            raise ServerStartError(msg, _read_log_tail(log_path))

        try:
            resp = httpx.get(health_url, timeout=2.0)
            if resp.is_success:
                return ServerProcess(
                    base_url=base_url,
                    process=proc,
                    log_file=log_file,
                )
        except httpx.ConnectError:
            pass

        time.sleep(poll_interval)

    proc.kill()
    proc.wait()
    log_file.close()
    msg = f"isola-server did not become healthy within {timeout}s"
    raise ServerStartError(msg, _read_log_tail(log_path))


def _read_log_tail(path: Path, lines: int = 30) -> str:
    try:
        all_lines = path.read_text(encoding="utf-8").splitlines()
        tail = all_lines[-lines:]
        return "--- server log (last {} lines) ---\n{}".format(
            len(tail), "\n".join(tail)
        )
    except OSError:
        return "(could not read server log)"
