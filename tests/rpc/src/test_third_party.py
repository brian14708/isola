from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from betterproto.lib.google.protobuf import NullValue
from stub.promptkit.script import v1 as pb

if TYPE_CHECKING:
    import pathlib

# Define test methods available in websocket.py
tests = [
    "pillow",
    "numpy",
    "pydantic",
    "tzdata",
]


@pytest.mark.asyncio
@pytest.mark.parametrize("method", tests)
async def test_websocket(
    client: pb.ScriptServiceStub, datadir: pathlib.Path, method: str
) -> None:
    """Test WebSocket functionality via script execution in WASI environment."""
    script_text = (datadir / "third_party.py").read_text()
    request = pb.ExecuteRequest(
        source=pb.Source(script_inline=pb.ScriptInline(script_text)),
        spec=pb.ExecutionSpec(
            method=method,
        ),
    )
    response = await client.execute(request)
    assert response.result.value.null_value == NullValue._
