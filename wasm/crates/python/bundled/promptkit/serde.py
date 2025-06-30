from typing import Any

import _promptkit_serde as _serde


def json_loads(data: str) -> Any:
    return _serde.json_loads(data)


def json_dumps(data: Any) -> str:
    return _serde.json_dumps(data)
