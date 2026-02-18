"""HTTP API client for integration tests."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Self

import httpx


@dataclass
class ExecuteResult:
    """Result of script execution."""

    value: Any | None = None
    error_code: str | None = None
    error_message: str | None = None

    @property
    def is_error(self) -> bool:
        return self.error_code is not None


@dataclass
class ExecuteResponse:
    """Response from execute endpoint."""

    result: ExecuteResult
    traces: list[dict[str, Any]]


class HttpClient:
    """HTTP client for isola-server API."""

    def __init__(self, base_url: str = "http://localhost:3000") -> None:
        self.base_url = base_url
        self._client = httpx.AsyncClient(base_url=base_url, timeout=60.0)

    async def close(self) -> None:
        await self._client.aclose()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *args: object) -> None:
        await self.close()

    async def execute(
        self,
        *,
        script: str,
        function: str = "main",
        args: list[Any] | None = None,
        kwargs: dict[str, Any] | None = None,
        timeout_ms: int | None = None,
        prelude: str = "",
        trace: bool = False,
        runtime: str = "python3",
    ) -> ExecuteResponse:
        """Execute a script and return the result."""
        request_body: dict[str, Any] = {
            "runtime": runtime,
            "script": script,
            "function": function,
            "args": args or [],
            "kwargs": kwargs or {},
            "trace": trace,
        }
        if prelude:
            request_body["prelude"] = prelude
        if timeout_ms is not None:
            request_body["timeout_ms"] = timeout_ms

        response = await self._client.post(
            "/api/v1/execute",
            json=request_body,
            headers={"Accept": "application/json"},
        )
        data = response.json()

        if "error" in data:
            return ExecuteResponse(
                result=ExecuteResult(
                    error_code=data["error"]["code"],
                    error_message=data["error"]["message"],
                ),
                traces=[],
            )

        return ExecuteResponse(
            result=ExecuteResult(value=data.get("result")),
            traces=data.get("traces", []),
        )
