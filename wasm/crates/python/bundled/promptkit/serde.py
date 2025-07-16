try:
    from _promptkit_serde import dumps, loads
except ImportError:
    import json
    import tomllib
    from typing import Any, Literal, overload

    import cbor2
    import tomli_w
    import yaml

    type Format = Literal["json", "yaml", "toml", "cbor"]

    @overload
    def _dumps(obj: Any, format: Literal["json"]) -> str: ...
    @overload
    def _dumps(obj: Any, format: Literal["yaml"]) -> str: ...
    @overload
    def _dumps(obj: Any, format: Literal["toml"]) -> str: ...
    @overload
    def _dumps(obj: Any, format: Literal["cbor"]) -> bytes: ...
    def _dumps(obj: Any, format: Format) -> str | bytes:
        if format == "json":
            return json.dumps(obj)
        elif format == "yaml":
            return yaml.dump(obj)
        elif format == "toml":
            return tomli_w.dumps(obj)
        elif format == "cbor":
            return cbor2.dumps(obj)
        else:
            raise ValueError(f"Unsupported format: {format}")

    @overload
    def _loads(s: str, format: Literal["json"]) -> Any: ...
    @overload
    def _loads(s: str, format: Literal["yaml"]) -> Any: ...
    @overload
    def _loads(s: str, format: Literal["toml"]) -> Any: ...
    @overload
    def _loads(s: bytes | bytearray, format: Literal["cbor"]) -> Any: ...
    def _loads(s: str | bytes | bytearray, format: Format) -> Any:
        if format == "json":
            return json.loads(s)
        elif format == "yaml":
            return yaml.safe_load(s)
        elif format == "toml":
            return tomllib.loads(s if isinstance(s, str) else s.decode())
        elif format == "cbor":
            if isinstance(s, str):
                raise ValueError("CBOR format requires bytes input")
            return cbor2.loads(s)
        else:
            raise ValueError(f"Unsupported format: {format}")

    dumps = _dumps
    loads = _loads


__all__ = ["dumps", "loads"]
