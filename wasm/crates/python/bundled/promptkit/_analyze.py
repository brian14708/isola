import inspect
import json
import typing

from pydantic import TypeAdapter


def analyze(ctx, req):
    ret = {"method_infos": []}
    for m in req["methods"]:
        fn = ctx.get(m)
        if not fn or not callable(fn):
            continue
        ret["method_infos"].append(analyze_function(fn))
    return ret


def analyze_function(fn):
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


def type_to_schema(typ):
    if isinstance(typ, typing.Iterable | typing.AsyncIterable):
        schema = TypeAdapter(typ.__args__[0]).json_schema()
        schema["promptkit"] = {"stream": True}
    else:
        schema = TypeAdapter(typ).json_schema()
    return json.dumps(schema)
