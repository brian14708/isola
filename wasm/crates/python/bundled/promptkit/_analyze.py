import inspect
import json
import typing
from collections.abc import AsyncIterable, Iterable

from pydantic import TypeAdapter


def analyze(ctx: dict[str, object], req: dict[str, str]) -> dict[str, object]:
    method_infos: list[dict[str, object]] = []
    for m in req["methods"]:
        fn = ctx.get(m)
        if not fn or not callable(fn):
            continue
        method_infos.append(analyze_function(fn))
    return {
        "method_infos": method_infos,
    }


def analyze_function[T](fn: typing.Callable[..., T]) -> dict[str, object]:
    sig = inspect.signature(fn)
    return {
        "name": fn.__name__,
        "description": fn.__doc__ or "",
        "argument_types": [
            {
                "name": name,
                "json_schema": (
                    type_to_schema(param.annotation)
                    if param.annotation != inspect.Parameter.empty
                    else None
                ),
            }
            for name, param in sig.parameters.items()
        ],
        "result_type": {
            "json_schema": (
                type_to_schema(sig.return_annotation)
                if sig.return_annotation != inspect.Signature.empty
                else None
            )
        },
    }


def type_to_schema(typ: object) -> str:
    if isinstance(typ, Iterable | AsyncIterable):
        schema = TypeAdapter(typing.cast("type", typing.get_args(typ)[0])).json_schema()
        schema["promptkit"] = {"stream": True}
    else:
        schema = TypeAdapter(typ).json_schema()
    return json.dumps(schema)
