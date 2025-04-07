from typing import Literal

import _promptkit_sys as _sys

class Connection:
    def recv(
        self,
    ) -> (
        tuple[Literal[False], None, None]
        | tuple[Literal[True], str | bytes, None]
        | tuple[Literal[True], None, _sys.Pollable[None]]
    ): ...
    def send(self, data: str | bytes) -> _sys.Pollable[None]: ...
    def close(self) -> None: ...
    def shutdown(self) -> None: ...

def connect(
    url: str, metadata: dict[str, str] | None, timeout: float | None
) -> _sys.Pollable[Connection]: ...
