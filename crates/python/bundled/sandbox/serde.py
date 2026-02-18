try:
    from _isola_serde import dumps, loads
except ImportError:
    import json
    from typing import Literal, cast, overload

    import cbor2
    import yaml

    type Format = Literal["json", "yaml", "cbor"]

    @overload
    def _dumps(obj: object, format_: Literal["json"], /) -> str: ...
    @overload
    def _dumps(obj: object, format_: Literal["yaml"], /) -> str: ...
    @overload
    def _dumps(obj: object, format_: Literal["cbor"], /) -> bytes: ...
    def _dumps(obj: object, format_: str, /) -> str | bytes:
        if format_ == "json":
            return json.dumps(obj)
        if format_ == "yaml":
            return yaml.dump(obj)
        if format_ == "cbor":
            return cbor2.dumps(obj)
        msg = f"Unsupported format: {format_}"
        raise ValueError(msg)

    @overload
    def _loads(s: str, format_: Literal["json"], /) -> object: ...
    @overload
    def _loads(s: str, format_: Literal["yaml"], /) -> object: ...
    @overload
    def _loads(s: bytes, format_: Literal["cbor"], /) -> object: ...
    def _loads(s: str | bytes, format_: Format | str, /) -> object:
        if format_ == "json":
            return cast("object", json.loads(s))
        if format_ == "yaml":
            return cast("object", yaml.safe_load(s))
        if format_ == "cbor":
            if isinstance(s, str):
                msg = "CBOR format requires bytes input"
                raise ValueError(msg)
            return cast("object", cbor2.loads(s))
        msg = f"Unsupported format: {format_}"
        raise ValueError(msg)

    dumps = _dumps
    loads = _loads


__all__ = ["dumps", "loads"]
