import io
import time
from typing import Any, cast

from promptkit import http


def simple(httpbin_url: str) -> None:
    k = str(time.time())
    result = cast(
        "Any",
        http.get(httpbin_url + "/get", params={"value": k}, headers={"x-my-test": k}),
    )
    assert result["args"]["value"] == k
    assert result["headers"]["X-My-Test"] == k

    result = cast(
        "Any",
        http.post(httpbin_url + "/post", data={"value": k}, headers={"x-my-test": k}),
    )
    assert result["headers"]["X-My-Test"] == k
    assert result["json"]["value"] == k


def error(httpbin_url: str) -> None:
    exc = ""
    try:
        http.get(httpbin_url + "/status/503")
    except Exception as e:
        exc = str(e)
    assert "503" in exc
    exc = ""
    try:
        http.post(httpbin_url + "/status/500", data={"value": "test"})
    except Exception as e:
        exc = str(e)
    assert "500" in exc


async def multipart(httpbin_url: str) -> None:
    async with http.fetch(
        "POST",
        httpbin_url + "/post",
        files={
            "file": b"test",
            "file2": ("a.txt", io.BytesIO(b"test2"), "text/plain"),
        },
    ) as r:
        result = cast("Any", await r.ajson())
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
