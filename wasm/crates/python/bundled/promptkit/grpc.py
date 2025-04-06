import _promptkit_grpc
import _promptkit_rpc

from promptkit.asyncio import subscribe

__all__ = [
    "client",
]


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

    def __init__(self, pool, url, request_type, response_type, metadata, timeout):
        self.pool = pool
        self.url = url
        self.request_type = request_type
        self.response_type = response_type
        self.metadata = metadata
        self.timeout = timeout
        self.conn = None

    def _conn(self):
        return _promptkit_rpc.connect(self.url, self.metadata, self.timeout)

    async def __aenter__(self):
        self.conn = GRPCClient(
            self.pool,
            await subscribe(self._conn()),
            self.request_type,
            self.response_type,
        )
        return self.conn

    async def __aexit__(self, _type, _value, _trace):
        if self.conn:
            self.conn.shutdown()

    def __enter__(self):
        self.conn = GRPCClient(
            self.pool, self._conn().wait(), self.request_type, self.response_type
        )
        return self.conn

    def __exit__(self, _type, _value, _trace):
        self.conn.shutdown()


class GRPCClient:
    __slots__ = ("pool", "conn", "request_type", "response_type")

    def __init__(self, pool, conn, request_type, response_type):
        self.pool = pool
        self.conn = conn
        self.request_type = request_type
        self.response_type = response_type

    def shutdown(self):
        self.conn.shutdown()

    def close(self):
        self.conn.close()

    async def arecv(self):
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                await subscribe(poll)
            else:
                return self.pool.decode(self.response_type, value)

    async def arecv_streaming(self):
        while (value := await self.arecv()) is not None:
            yield value

    def recv(self):
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                poll.wait()
            else:
                return self.pool.decode(self.response_type, value)

    def recv_streaming(self):
        while (value := self.recv()) is not None:
            yield value

    async def asend(self, value):
        value = self.pool.encode(self.request_type, value)
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                await subscribe(poll)
            else:
                break

    def send(self, value):
        value = self.pool.encode(self.request_type, value)
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                poll.wait()
            else:
                break


class Service:
    def __init__(self, url, service, metadata=None, timeout=None):
        self.url = url
        self.service = service
        self.pool, self.methods = _promptkit_grpc.grpc_reflection(
            url, service, metadata, timeout
        )

    def stream(self, method, metadata=None, timeout=None):
        i, o = self.methods[method]
        return GRPCRequest(
            self.pool, f"{self.url}/{self.service}/{method}", i, o, metadata, timeout
        )

    def call(self, method, req, **kwargs):
        with self.stream(method, **kwargs) as r:
            r.send(req)
            r.close()
            return r.recv()

    async def acall(self, method, req, **kwargs):
        async with self.stream(method, **kwargs) as r:
            await r.asend(req)
            r.close()
            return await r.arecv()


def client(url, service, metadata=None, timeout=None):
    return Service(url, service, metadata, timeout)
