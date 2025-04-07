from typing import TYPE_CHECKING, final

from _promptkit_grpc import grpc_reflection
from _promptkit_rpc import connect

from promptkit.asyncio import subscribe

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Generator

    import _promptkit_sys as _sys
    from _promptkit_grpc import Descriptor
    from _promptkit_rpc import Connection

__all__ = ["client"]


@final
class GRPCRequest:
    __slots__ = (
        "pool",
        "url",
        "request_type",
        "response_type",
        "metadata",
        "conn",
        "timeout",
    )

    def __init__(
        self,
        pool: "Descriptor",
        url: str,
        request_type: str,
        response_type: str,
        metadata: dict[str, str] | None,
        timeout: float | None,
    ):
        self.pool = pool
        self.url = url
        self.request_type = request_type
        self.response_type = response_type
        self.metadata = metadata
        self.timeout = timeout
        self.conn: GRPCClient | None = None

    def _conn(self) -> "_sys.Pollable[Connection]":
        return connect(self.url, self.metadata, self.timeout)

    async def __aenter__(self) -> "GRPCClient":
        self.conn = GRPCClient(
            self.pool,
            await subscribe(self._conn()),
            self.request_type,
            self.response_type,
        )
        return self.conn

    async def __aexit__(self, *_: object) -> None:
        if self.conn:
            self.conn.shutdown()

    def __enter__(self) -> "GRPCClient":
        self.conn = GRPCClient(
            self.pool, self._conn().wait(), self.request_type, self.response_type
        )
        return self.conn

    def __exit__(self, *_: object) -> None:
        if self.conn:
            self.conn.shutdown()


@final
class GRPCClient:
    __slots__ = ("pool", "conn", "request_type", "response_type")

    def __init__(
        self,
        pool: "Descriptor",
        conn: "Connection",
        request_type: str,
        response_type: str,
    ):
        self.pool = pool
        self.conn = conn
        self.request_type = request_type
        self.response_type = response_type

    def shutdown(self) -> None:
        self.conn.shutdown()

    def close(self) -> None:
        self.conn.close()

    async def arecv(self) -> object:
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                await subscribe(poll)
            elif isinstance(value, bytes):
                return self.pool.decode(self.response_type, value)

    async def arecv_streaming(self) -> "AsyncGenerator[object]":
        while (value := await self.arecv()) is not None:
            yield value

    def recv(self) -> object:
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                poll.wait()
            elif isinstance(value, bytes):
                return self.pool.decode(self.response_type, value)

    def recv_streaming(self) -> "Generator[object]":
        while (value := self.recv()) is not None:
            yield value

    async def asend(self, value: object) -> None:
        value = self.pool.encode(self.request_type, value)
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                await subscribe(poll)
            else:
                break

    def send(self, value: object) -> None:
        value = self.pool.encode(self.request_type, value)
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                poll.wait()
            else:
                break


class Service:
    def __init__(
        self,
        url: str,
        service: str,
        *,
        metadata: dict[str, str] | None = None,
        timeout: float | None = None,
    ):
        self.url: str = url
        self.service: str = service
        pool, methods = grpc_reflection(url, service, metadata, timeout)
        self.pool: Descriptor = pool
        self.methods: dict[str, tuple[str, str]] = methods

    def stream(
        self,
        method: str,
        *,
        metadata: dict[str, str] | None = None,
        timeout: float | None = None,
    ) -> GRPCRequest:
        i, o = self.methods[method]
        return GRPCRequest(
            self.pool, f"{self.url}/{self.service}/{method}", i, o, metadata, timeout
        )

    def call(
        self,
        method: str,
        req: object,
        *,
        metadata: dict[str, str] | None = None,
        timeout: float | None = None,
    ) -> object:
        with self.stream(method, metadata=metadata, timeout=timeout) as r:
            r.send(req)
            r.close()
            return r.recv()

    async def acall(
        self,
        method: str,
        req: object,
        *,
        metadata: dict[str, str] | None = None,
        timeout: float | None = None,
    ) -> object:
        async with self.stream(method, metadata=metadata, timeout=timeout) as r:
            await r.asend(req)
            r.close()
            return await r.arecv()


def client(
    url: str,
    service: str,
    *,
    metadata: dict[str, str] | None = None,
    timeout: float | None = None,
) -> Service:
    return Service(url, service, metadata=metadata, timeout=timeout)
