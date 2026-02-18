from __future__ import annotations

import os
import pathlib
from typing import TYPE_CHECKING

import pytest
import pytest_asyncio
from fixtures.websocket import WebSocketTestServer
from http_client import HttpClient

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator


@pytest_asyncio.fixture
async def client() -> AsyncGenerator[HttpClient, None]:
    base_url = os.getenv("ISOLA_BASE_URL", "http://localhost:3000")
    async with HttpClient(base_url) as c:
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
