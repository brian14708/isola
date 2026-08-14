from typing import Literal, overload

from _isola_serde import dumps as _dumps
from _isola_serde import loads as _loads

type SerializationFormat = Literal["json", "yaml", "cbor"]


@overload
def dumps(obj: object, format: Literal["json"], /) -> str: ...


@overload
def dumps(obj: object, format: Literal["yaml"], /) -> str: ...


@overload
def dumps(obj: object, format: Literal["cbor"], /) -> bytes: ...


def dumps(obj: object, format_: SerializationFormat, /) -> str | bytes:
    if format_ == "cbor":
        return _dumps(obj, "cbor")
    return _dumps(obj, format_)


@overload
def loads(s: str, format: Literal["json"], /) -> object: ...


@overload
def loads(s: str, format: Literal["yaml"], /) -> object: ...


@overload
def loads(s: bytes, format: Literal["cbor"], /) -> object: ...


def loads(s: str | bytes, format_: SerializationFormat, /) -> object:
    if format_ == "cbor":
        if not isinstance(s, bytes):
            msg = "CBOR format requires bytes input"
            raise TypeError(msg)
        return _loads(s, "cbor")
    if not isinstance(s, str):
        msg = f"{format_.upper()} format requires string input"
        raise TypeError(msg)
    return _loads(s, format_)


__all__ = ["SerializationFormat", "dumps", "loads"]
