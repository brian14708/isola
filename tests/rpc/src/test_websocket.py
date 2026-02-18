from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    import pathlib

    from http_client import HttpClient

websocket_tests = [
    "simple_echo",
    "async_echo",
    "binary_echo",
    "connection_close",
    "connection_close_error_code",
    "timeout_test",
    "concurrent_connections",
    "concurrent_messages",
]


@pytest.mark.asyncio
@pytest.mark.parametrize("method", websocket_tests)
async def test_websocket(
    websocket_server, client: HttpClient, datadir: pathlib.Path, method: str
) -> None:
    script_text = (datadir / "websocket.py").read_text()
    response = await client.execute(
        script=script_text,
        function=method,
        args=[websocket_server.url],
    )
    assert not response.result.is_error
    assert response.result.value is None
