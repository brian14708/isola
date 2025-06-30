import pytest
from stub.promptkit.script import v1 as pb


@pytest.mark.asyncio
async def test_runtime(client: pb.ScriptServiceStub) -> None:
    response = await client.list_runtime(pb.ListRuntimeRequest())
    assert "python3" in [r.name for r in response.runtimes]
