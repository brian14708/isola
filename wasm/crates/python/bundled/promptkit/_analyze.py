import typing
import inspect
import json

from pydantic import TypeAdapter


def analyze(ctx, dict):
    import time

    s = time.time()
    ret = {"method_infos": []}
    for m in dict["methods"]:
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
    try:
        if isinstance(typ, typing.Iterable) or isinstance(typ, typing.AsyncIterable):
            schema = TypeAdapter(typ.__args__[0]).json_schema()
            schema["promptkit"] = {"stream": True}
        else:
            schema = TypeAdapter(typ).json_schema()
        return json.dumps(schema)
    except:
        return None
