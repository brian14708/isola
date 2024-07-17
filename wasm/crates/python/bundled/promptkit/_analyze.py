import typing
import inspect
import json

from apischema.json_schema import deserialization_schema, serialization_schema


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
                    type_to_schema(param.annotation, deserialization_schema)
                    if param.annotation != inspect.Parameter.empty
                    else None
                ),
            }
            for name, param in sig.parameters.items()
        ],
        "result_type": {
            "json_schema": (
                type_to_schema(sig.return_annotation, serialization_schema)
                if sig.return_annotation != inspect.Signature.empty
                else None
            )
        },
    }


def type_to_schema(typ, to_schema):
    try:
        if isinstance(typ, typing.Iterable) or isinstance(typ, typing.AsyncIterable):
            schema = to_schema(typ.__args__[0])
            schema["promptkit"] = {"stream": True}
        else:
            schema = to_schema(typ)
        return json.dumps(schema)
    except:
        return None


if __name__ == "__main__":

    def test(a: int, b: typing.Iterable[int]) -> str:
        """this is a test function"""
        ...

    print(analyze(locals(), {"method_infos": ["test", "nonexist"]}))
