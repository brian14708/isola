from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    import pathlib

    from http_client import HttpClient


@pytest.mark.asyncio
async def test_simple(client: HttpClient, datadir: pathlib.Path) -> None:
    response = await client.execute(
        script=(datadir / "basic.py").read_text(),
        function="add",
        args=[2, 3],
    )
    assert response.result.value == (2 + 3)


@pytest.mark.asyncio
async def test_named_argument(client: HttpClient, datadir: pathlib.Path) -> None:
    response = await client.execute(
        script=(datadir / "basic.py").read_text(),
        function="add",
        args=[2, 3],
        kwargs={"c": 5},
    )
    assert response.result.value == (2 + 3 * 5)


@pytest.mark.asyncio
async def test_async(client: HttpClient, datadir: pathlib.Path) -> None:
    response = await client.execute(
        script=(datadir / "basic.py").read_text(),
        function="async_add",
        args=[2, 3],
    )
    assert response.result.value == (2 + 3)


@pytest.mark.asyncio
async def test_error(client: HttpClient, datadir: pathlib.Path) -> None:
    response = await client.execute(
        script=(datadir / "basic.py").read_text(),
        function="raise_exception",
        args=["Hello"],
    )
    assert response.result.is_error
    assert response.result.error_code == "SCRIPT_ERROR"
    assert "Hello" in (response.result.error_message or "")


@pytest.mark.asyncio
async def test_timeout(client: HttpClient, datadir: pathlib.Path) -> None:
    response = await client.execute(
        script=(datadir / "basic.py").read_text(),
        function="stall",
        timeout_ms=1,
    )
    assert response.result.is_error
    assert response.result.error_code == "TIMEOUT"
