from __future__ import annotations

import inspect
from typing import TYPE_CHECKING, Any

import pytest

if TYPE_CHECKING:
    import pathlib

    from http_client import HttpClient

all_tests = [
    "simple",
    "error",
    "multipart",
    "read_twice",
]


@pytest.mark.asyncio
@pytest.mark.parametrize("method", all_tests)
async def test_httpbin(
    httpbin, client: HttpClient, datadir: pathlib.Path, method: str
) -> None:
    script_text = (datadir / "http.py").read_text()
    response = await client.execute(
        script=script_text,
        function=method,
        args=[httpbin.url],
    )
    assert not response.result.is_error
    assert response.result.value is None


@pytest.mark.asyncio
@pytest.mark.parametrize("method", all_tests)
async def test_httpbin_local(httpbin, datadir: pathlib.Path, method: str) -> None:
    scope: dict[str, Any] = {}
    exec((datadir / "http.py").read_text(), scope)  # noqa: S102
    fn = scope[method]
    if inspect.iscoroutinefunction(fn):
        ret = await fn(httpbin.url)
    else:
        ret = fn(httpbin.url)
    assert ret is None
