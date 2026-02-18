import os

import pytest
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client


@pytest.mark.asyncio
async def test_run() -> None:
    base_url = os.getenv("PROMPTKIT_BASE_URL", "http://localhost:3000")
    async with (
        streamable_http_client(f"{base_url}/mcp") as (
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
