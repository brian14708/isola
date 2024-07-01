import asyncio

import _promptkit_http as _http
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
        self.resp.close()
        self.resp = None

    # async read methods

    async def _aread(self, encoding):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf)) is not None:
            await subscribe(poll)
        return buf.read_all()

    async def ajson(self):
        return await self._aread("json")

    async def atext(self):
        return await self._aread("text")

    async def aread(self):
        return await self._aread("bytes")

    # async iterator methods

    async def _aiter(self, encoding):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf)) is not None:
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

    def _read(self, encoding):
        return self.resp.blocking_read(encoding)

    def read(self):
        return self._read("bytes")

    def json(self):
        return self._read("json")

    def text(self):
        return self._read("text")

    # sync iterator methods

    def _iter(self, encoding):
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf)) is not None:
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


def fetch(method, url, *, params=None, headers=None, body=None, timeout=None):
    return Request(method, url, params, headers, body, timeout)


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
            if event.data == "[DONE]":
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
