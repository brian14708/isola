from typing import cast

from promptkit import grpc


def call_grpc():
    hello = grpc.client("grpc://localhost:3000", "promptkit.script.v1.ScriptService")
    resp = cast(
        "dict[str, list[object]]",
        hello.call(
            "ListRuntime",
            {},
        ),
    )
    assert len(resp["runtimes"]) == 1
