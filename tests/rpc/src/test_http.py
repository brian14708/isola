from __future__ import annotations

import asyncio
from typing import TYPE_CHECKING, Any

import pytest
from betterproto.lib.google.protobuf import NullValue, Value
from stub.promptkit.script import v1 as pb

if TYPE_CHECKING:
    import pathlib

all_tests = [
    "simple",
    "error",
    "multipart",
    "read_twice",
]


@pytest.mark.asyncio
@pytest.mark.parametrize("method", all_tests)
async def test_httpbin(
    httpbin, client: pb.ScriptServiceStub, datadir: pathlib.Path, method: str
) -> None:
    script_text = (datadir / "http.py").read_text()
    request = pb.ExecuteRequest(
        source=pb.Source(script_inline=pb.ScriptInline(script_text)),
        spec=pb.ExecutionSpec(
            method=method,
            arguments=[pb.Argument(value=Value(string_value=httpbin.url))],
        ),
    )
    response = await client.execute(request)
    assert response.result.value.null_value == NullValue._


@pytest.mark.asyncio
@pytest.mark.parametrize("method", all_tests)
async def test_httpbin_local(httpbin, datadir: pathlib.Path, method: str) -> None:
    scope: dict[str, Any] = {}
    exec((datadir / "http.py").read_text(), scope)
    fn = scope[method]
    if asyncio.iscoroutinefunction(fn):
        ret = await fn(httpbin.url)
    else:
        ret = fn(httpbin.url)
    assert ret is None
