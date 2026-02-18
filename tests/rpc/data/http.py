import io
import time
from typing import cast

from sandbox import http


async def simple(httpbin_url: str) -> None:
    k = str(time.time())

    async with http.fetch(
        "GET",
        httpbin_url + "/get",
        params={"value": k},
        headers={"x-my-test": k},
    ) as r:
        result = cast("dict[str, dict[str, str]]", await r.ajson())
    assert result["args"]["value"] == k
    assert result["headers"]["X-My-Test"] == k

    async with http.fetch(
        "POST",
        httpbin_url + "/post",
        body={"value": k},
        headers={"x-my-test": k},
    ) as r:
        result = cast("dict[str, dict[str, str]]", await r.ajson())
    assert result["headers"]["X-My-Test"] == k
    assert result["json"]["value"] == k


async def error(httpbin_url: str) -> None:
    async with http.fetch("GET", httpbin_url + "/status/503") as r:
        assert r.status == 503

    async with http.fetch(
        "POST",
        httpbin_url + "/status/500",
        body={"value": "test"},
    ) as r:
        assert r.status == 500


async def multipart(httpbin_url: str) -> None:
    async with http.fetch(
        "POST",
        httpbin_url + "/post",
        files={
            "file": b"test",
            "file2": ("a.txt", io.BytesIO(b"test2"), "text/plain"),
        },
    ) as r:
        result = cast("dict[str, dict[str, str]]", await r.ajson())
        assert result["files"]["file"] == "test"
        assert result["files"]["file2"] == "test2"


async def read_twice(httpbin_url: str) -> None:
    async with http.fetch("GET", httpbin_url + "/get") as r:
        await r.ajson()
        exc = ""
        try:
            await r.ajson()
        except Exception as e:
            exc = str(e)
        assert "Response already read" in exc
