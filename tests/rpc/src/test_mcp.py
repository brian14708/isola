from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

if TYPE_CHECKING:
    from fixtures.server import ServerProcess


@pytest.mark.asyncio
async def test_run(server: ServerProcess) -> None:
    async with (
        streamable_http_client(f"{server.base_url}/mcp") as (
            read_stream,
            write_stream,
            _,
        ),
        ClientSession(read_stream, write_stream) as session,
    ):
        # Initialize the connection
        _ = await session.initialize()
        # List available tools
        tools = await session.list_tools()
        assert len(tools.tools) > 0
