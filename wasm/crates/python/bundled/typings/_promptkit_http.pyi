from typing import Any

def fetch(
    method: str,
    url: str,
    params: dict[str, str] | None,
    headers: dict[str, str] | None,
    body: Any | bytes | None,
    timeout: float | None,
) -> Any: ...
