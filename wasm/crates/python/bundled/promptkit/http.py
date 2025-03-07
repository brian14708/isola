import asyncio

import _promptkit_http as _http
import _promptkit_rpc
from promptkit.asyncio import subscribe, run as asyncio_run


class Request:
    __slots__ = (
        "method",
        "url",
        "params",
        "headers",
        "body",
        "timeout",
        "extra",
        "resp",
    )

    def __init__(self, method, url, params, headers, body, timeout, extra=None):
        self.method = method
        self.url = url
        self.params = params
        self.headers = headers
        self.body = body
        self.timeout = timeout
        self.extra = extra
        self.resp = None

    def _fetch(self):
        req = _http.fetch(
            self.method, self.url, self.params, self.headers, self.body, self.timeout
        )
        self.body = None
        return req

    async def __aenter__(self):
        self.resp = Response(await subscribe(self._fetch()))
        return self.resp

    async def __aexit__(self, _type, _value, _trace):
        self.resp.close()

    def __enter__(self):
        self.resp = Response(self._fetch().wait())
        return self.resp

    def __exit__(self, _type, _value, _trace):
        self.resp.close()


class ServerSentEvent:
    __slots__ = ("id", "event", "data")

    def __init__(self, id, event, data):
        self.id = id
        self.event = event
        self.data = data


class Response:
    __slots__ = ("resp", "_status", "_headers")

    def __init__(self, resp):
        self.resp = resp
        self._status = None
        self._headers = None

    @property
    def status(self):
        if self._status is None:
            self._status = self.resp.status()
        return self._status

    @property
    def headers(self):
        if self._headers is None:
            self._headers = self.resp.headers()
        return self._headers

    def close(self):
        if self.resp:
            self.resp.close()
            self.resp = None

    # async read methods

    async def _aread(self, encoding, size):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, size)) is not None:
            await subscribe(poll)
        return buf.read_all()

    async def ajson(self):
        return await self._aread("json", -1)

    async def atext(self):
        return await self._aread("text", -1)

    async def aread(self, size=-1):
        return await self._aread("bytes", size)

    # async iterator methods

    async def _aiter(self, encoding):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, 16384)) is not None:
            while (data := buf.next()) is not None:
                yield data
            await subscribe(poll)
        while (data := buf.next()) is not None:
            yield data

    async def aiter_bytes(self):
        async for data in self._aiter("bytes"):
            yield data

    async def aiter_lines(self):
        async for line in self._aiter("lines"):
            yield line

    async def aiter_sse(self):
        async for id, event, data in self._aiter("sse"):
            yield ServerSentEvent(id, event, data)

    # sync read methods

    def _read(self, encoding, size):
        return self.resp.blocking_read(encoding, size)

    def read(self, size=-1):
        return self._read("bytes", size)

    def json(self):
        return self._read("json", -1)

    def text(self):
        return self._read("text", -1)

    # sync iterator methods

    def _iter(self, encoding):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, 16384)) is not None:
            while (data := buf.next()) is not None:
                yield data
            poll.wait()
        while (data := buf.next()) is not None:
            yield data

    def iter_bytes(self):
        return self._iter("bytes")

    def iter_lines(self):
        return self._iter("lines")

    def iter_sse(self):
        for id, event, data in self._iter("sse"):
            yield ServerSentEvent(id, event, data)


class WebSocketRequest:
    __slots__ = ("url", "headers", "conn", "timeout")

    def __init__(self, url, headers, timeout):
        self.url = url
        self.headers = headers
        self.timeout = timeout
        self.conn = None

    def _conn(self):
        req = _promptkit_rpc.connect(self.url, self.headers, self.timeout)
        return req

    async def __aenter__(self):
        self.conn = Websocket(await subscribe(self._conn()))
        return self.conn

    async def __aexit__(self, _type, _value, _trace):
        self.conn.shutdown()

    def __enter__(self):
        self.conn = Websocket(self._conn().wait())
        return self.conn

    def __exit__(self, _type, _value, _trace):
        self.conn.shutdown()


class Websocket:
    __slots__ = ("conn",)

    def __init__(self, conn):
        self.conn = conn

    def shutdown(self):
        self.conn.shutdown()

    def close(self):
        self.conn.close()

    async def arecv(self):
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            elif poll is not None:
                await subscribe(poll)
            else:
                return value

    async def arecv_streaming(self):
        while True:
            value = await self.arecv()
            if value is None:
                return
            yield value

    def recv(self):
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            elif poll is not None:
                poll.wait()
            else:
                return value

    def recv_streaming(self):
        while (value := self.recv()) is not None:
            yield value

    async def asend(self, value):
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                await subscribe(poll)
            else:
                break

    def send(self, value):
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                poll.wait()
            else:
                break


def fetch(method, url, *, params=None, headers=None, body=None, timeout=None):
    return Request(method, url, params, headers, body, timeout)


def ws_connect(url, *, headers=None, timeout=None):
    return WebSocketRequest(url, headers, timeout)


### Legacy API


def _validate_status(resp):
    if not 200 <= resp.status < 300:
        raise RuntimeError(f"http status check failed, status={resp.status}")


def get(
    url,
    params=None,
    headers=None,
    timeout=None,
    response="json",
    validate_status=True,
):
    with Request("GET", url, params, headers, None, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return resp._read(response)


def get_async(
    url,
    params=None,
    headers=None,
    timeout=None,
    response="json",
    validate_status=True,
):
    return Request(
        "GET",
        url,
        params,
        headers,
        None,
        timeout,
        {
            "type": response,
            "validate": validate_status,
        },
    )


def get_sse(
    url,
    params=None,
    headers=None,
    timeout=None,
):
    with Request(
        "GET",
        url,
        params,
        headers,
        None,
        timeout,
    ) as resp:
        _validate_status(resp)
        for event in resp.iter_sse():
            if event.data == "[DONE]":
                break
            yield _http.loads_json(event.data)


def post(
    url, data=None, headers=None, timeout=None, response="json", validate_status=True
):
    with Request("POST", url, None, headers, data, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return resp._read(response)


def post_async(
    url, data=None, headers=None, timeout=None, response="json", validate_status=True
):
    return Request(
        "POST",
        url,
        None,
        headers,
        data,
        timeout,
        {
            "type": response,
            "validate": validate_status,
        },
    )


def post_sse(url, data=None, headers=None, timeout=None):
    with Request("POST", url, None, headers, data, timeout) as resp:
        _validate_status(resp)
        for event in resp.iter_sse():
            if event.data.startswith("[DONE]"):
                break
            yield _http.loads_json(event.data)


async def _fetch(r, ignore_error):
    extra = r.extra or {}
    try:
        async with r as resp:
            if r.extra.get("validate"):
                _validate_status(resp)
            return await resp._aread(extra.get("type", "json"))
    except Exception as e:
        if ignore_error:
            return e
        raise e


async def _fetch_all(requests, ignore_error):
    return await asyncio.gather(*[_fetch(r, ignore_error) for r in requests])


def fetch_all(requests, ignore_error=False):
    return asyncio_run(_fetch_all(requests, ignore_error))
