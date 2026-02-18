from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

if TYPE_CHECKING:
    from http_client import HttpClient


@pytest.mark.asyncio
async def test_unknown_runtime_rejected(client: HttpClient) -> None:
    response = await client.execute(
        runtime="python-nope",
        script="def main():\n    return 1\n",
    )
    assert response.result.is_error
    assert response.result.error_code == "UNKNOWN_RUNTIME"
