from typing import TYPE_CHECKING

import pytest
from betterproto.lib.google.protobuf import NullValue, Value
from stub.promptkit.script import v1 as pb

if TYPE_CHECKING:
    import pathlib

# Define test methods available in websocket.py
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
    websocket_server, client: pb.ScriptServiceStub, datadir: "pathlib.Path", method: str
) -> None:
    """Test WebSocket functionality via script execution in WASI environment."""
    script_text = (datadir / "websocket.py").read_text()
    request = pb.ExecuteRequest(
        source=pb.Source(script_inline=pb.ScriptInline(script_text)),
        spec=pb.ExecutionSpec(
            method=method,
            arguments=[pb.Argument(value=Value(string_value=websocket_server.url))],
        ),
    )
    response = await client.execute(request)
    assert response.result.value.null_value == NullValue._
