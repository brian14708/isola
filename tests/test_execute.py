import datetime
import json
from typing import TYPE_CHECKING

import pytest
from betterproto.lib.google.protobuf import Value

from .stub.promptkit.script import v1 as pb

if TYPE_CHECKING:
    import pathlib


@pytest.mark.asyncio
async def test_simple(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="add",
                arguments=[
                    pb.Argument(value=Value(number_value=2)),
                    pb.Argument(value=Value(number_value=3)),
                ],
            ),
        )
    )
    assert response.result.value.number_value == (2 + 3)


@pytest.mark.asyncio
async def test_named_argument(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="add",
                arguments=[
                    pb.Argument(name="c", value=Value(number_value=5)),
                    pb.Argument(value=Value(number_value=2)),
                    pb.Argument(value=Value(number_value=3)),
                ],
            ),
        )
    )
    assert response.result.value.number_value == (2 + 3 * 5)


@pytest.mark.asyncio
async def test_async(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="async_add",
                arguments=[
                    pb.Argument(value=Value(number_value=2)),
                    pb.Argument(value=Value(number_value=3)),
                ],
            ),
        )
    )
    assert response.result.value.number_value == (2 + 3)


@pytest.mark.asyncio
async def test_error(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="raise_exception",
                arguments=[pb.Argument(value=Value(string_value="Hello"))],
            ),
        )
    )
    assert response.result.error.code == pb.ErrorCode.GUEST_ABORTED
    assert "Hello" in response.result.error.message


@pytest.mark.asyncio
async def test_timeout(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.execute(
        pb.ExecuteRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            spec=pb.ExecutionSpec(
                method="stall",
                timeout=datetime.timedelta(milliseconds=1),
            ),
        )
    )
    assert response.result.error.code == pb.ErrorCode.DEADLINE_EXCEEDED


@pytest.mark.asyncio
async def test_analyze(client: pb.ScriptServiceStub, datadir: "pathlib.Path"):
    response = await client.analyze(
        pb.AnalyzeRequest(
            source=pb.Source(
                script_inline=pb.ScriptInline((datadir / "basic.py").read_text()),
            ),
            methods=["async_add"],
        )
    )
    method = response.analyze_result.method_infos[0]
    assert len(method.argument_types) == 3
    assert json.loads(method.result_type.json_schema)["type"] == "integer"
