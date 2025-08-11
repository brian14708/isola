try:
    from _promptkit_serde import dumps, loads
except ImportError:
    import json
    from typing import Literal, cast, overload

    import cbor2
    import yaml

    type Format = Literal["json", "yaml", "cbor"]

    @overload
    def _dumps(obj: object, format: Literal["json"]) -> str: ...
    @overload
    def _dumps(obj: object, format: Literal["yaml"]) -> str: ...
    @overload
    def _dumps(obj: object, format: Literal["cbor"]) -> bytes: ...
    def _dumps(obj: object, format: str) -> str | bytes:
        if format == "json":
            return json.dumps(obj)
        elif format == "yaml":
            return yaml.dump(obj)
        elif format == "cbor":
            return cbor2.dumps(obj)
        else:
            raise ValueError(f"Unsupported format: {format}")

    @overload
    def _loads(s: str, format: Literal["json"]) -> object: ...
    @overload
    def _loads(s: str, format: Literal["yaml"]) -> object: ...
    @overload
    def _loads(s: bytes, format: Literal["cbor"]) -> object: ...
    def _loads(s: str | bytes, format: Format | str) -> object:
        if format == "json":
            return cast("object", json.loads(s))
        elif format == "yaml":
            return cast("object", yaml.safe_load(s))
        elif format == "cbor":
            if isinstance(s, str):
                raise ValueError("CBOR format requires bytes input")
            return cast("object", cbor2.loads(s))
        else:
            raise ValueError(f"Unsupported format: {format}")

    dumps = _dumps
    loads = _loads


__all__ = ["dumps", "loads"]
