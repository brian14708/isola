from __future__ import annotations

import pathlib
from typing import TYPE_CHECKING

import pytest
import pytest_asyncio
from fixtures.websocket import WebSocketTestServer
from grpclib.client import Channel
from stub.promptkit.script.v1 import ScriptServiceStub

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator


@pytest_asyncio.fixture
async def client() -> AsyncGenerator[ScriptServiceStub, None]:
    async with Channel("localhost", port=3000) as channel:
        yield ScriptServiceStub(channel)


@pytest.fixture
def datadir() -> pathlib.Path:
    return pathlib.Path(__file__).parent / ".." / "data"


@pytest_asyncio.fixture
async def websocket_server() -> AsyncGenerator[WebSocketTestServer, None]:
    server = WebSocketTestServer()
    await server.start_server()
    yield server
    await server.stop_server()
