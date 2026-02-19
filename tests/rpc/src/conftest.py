from __future__ import annotations

import os
import pathlib
from typing import TYPE_CHECKING

import pytest
import pytest_asyncio
from fixtures.server import ServerProcess, start_server
from fixtures.websocket import WebSocketTestServer
from http_client import HttpClient

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Generator


@pytest.fixture(scope="session")
def server() -> Generator[ServerProcess, None, None]:
    """Start isola-server for the test session.

    If ``ISOLA_BASE_URL`` is set, skip spawning and use the external server.
    """
    external = os.getenv("ISOLA_BASE_URL")
    if external:
        yield ServerProcess(base_url=external)
        return

    srv = start_server()
    yield srv
    srv.stop()


@pytest_asyncio.fixture
async def client(server: ServerProcess) -> AsyncGenerator[HttpClient, None]:
    async with HttpClient(server.base_url) as c:
        yield c


@pytest.fixture
def datadir() -> pathlib.Path:
    return pathlib.Path(__file__).parent / ".." / "data"


@pytest_asyncio.fixture
async def websocket_server() -> AsyncGenerator[WebSocketTestServer, None]:
    server = WebSocketTestServer()
    await server.start_server()
    yield server
    await server.stop_server()
