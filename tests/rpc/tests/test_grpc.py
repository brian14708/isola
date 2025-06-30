from typing import TYPE_CHECKING

import pytest
from betterproto.lib.google.protobuf import NullValue
from stub.promptkit.script import v1 as pb

if TYPE_CHECKING:
    import pathlib


@pytest.mark.asyncio
async def test_simple(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "grpc.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="call_grpc",
                arguments=[],
            ),
        )
    )
    assert response.result.value.null_value == NullValue._
