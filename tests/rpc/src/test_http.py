from __future__ import annotations

from typing import TYPE_CHECKING

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
