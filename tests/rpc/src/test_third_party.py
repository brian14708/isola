from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    import pathlib

    from http_client import HttpClient

tests = [
    "pillow",
    "numpy",
    "pydantic",
    "tzdata",
]


@pytest.mark.asyncio
@pytest.mark.parametrize("method", tests)
async def test_third_party(
    client: HttpClient, datadir: pathlib.Path, method: str
) -> None:
    script_text = (datadir / "third_party.py").read_text()
    response = await client.execute(
        script=script_text,
        function=method,
    )
    assert not response.result.is_error
    assert response.result.value is None
