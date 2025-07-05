try:
    from _promptkit_serde import dumps, loads
except ImportError:
    import json
    from typing import Any, Literal

    import yaml

    type Format = Literal["json", "yaml"]

    def dumps(obj: Any, format: Format) -> str:
        if format == "json":
            return json.dumps(obj)
        elif format == "yaml":
            return yaml.dump(obj)
        else:
            raise ValueError(f"Unsupported format: {format}")

    def loads(s: str, format: Format) -> Any:
        if format == "json":
            return json.loads(s)
        elif format == "yaml":
            return yaml.safe_load(s)
        else:
            raise ValueError(f"Unsupported format: {format}")


__all__ = ["dumps", "loads"]
