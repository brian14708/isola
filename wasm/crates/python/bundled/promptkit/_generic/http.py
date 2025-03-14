import httpx
import json
import asyncio


class Request:
    __slots__ = ("request", "client", "response", "extra")

    def __init__(self, method, url, params, headers, body, timeout, extra=None):
        self.client = None
        self.response = None
        self.request = httpx.Request(
            method,
            url,
            params=params,
            headers=headers,
            content=body if type(body) == bytes else None,
            json=body if type(body) != bytes else None,
        )
        self.extra = extra

    async def __aenter__(self):
        self.client = httpx.AsyncClient()
        self.response = Response(await self.client.send(self.request, stream=True))
        return self.response

    async def __aexit__(self, _type, _value, _trace):
        await self.response.aclose()
        await self.client.aclose()

    def __enter__(self):
        self.client = httpx.Client()
        self.response = Response(self.client.send(self.request, stream=True))
        return self.response

    def __exit__(self, _type, _value, _trace):
        self.response.close()
        self.client.close()


class Response:
    def __init__(self, response):
        self.response = response

    def close(self):
        self.response.close()

    async def aclose(self):
        await self.response.aclose()

    @property
    def status(self):
        return self.response.status_code

    @property
    def headers(self):
        return self.response.headers

    async def aread(self):
        return await self.response.aread()

    def read(self):
        return self.response.read()

    async def atext(self):
        return (await self.response.aread()).decode()

    def text(self):
        return self.response.read().decode()

    async def ajson(self):
        return json.loads(await self.response.aread())

    def json(self):
        return json.loads(self.response.read())

    async def aiter_lines(self):
        async for line in self.response.aiter_lines():
            yield line

    def iter_lines(self):
        for line in self.response.iter_lines():
            yield line

    async def aiter_sse(self):
        decoder = SSEDecoder()
        async for line in self.response.aiter_lines():
            if event := decoder.decode(line):
                yield event

    def iter_sse(self):
        decoder = SSEDecoder()
        for line in self.response.iter_lines():
            if event := decoder.decode(line):
                yield event

    async def aiter_lines(self):
        async for line in self.response.aiter_lines():
            yield line

    def iter_lines(self):
        for line in self.response.iter_lines():
            yield line

    async def aiter_bytes(self):
        async for data in self.response.aiter_bytes():
            yield data

    def iter_bytes(self):
        for data in self.response.iter_bytes():
            yield data


class ServerSentEvent:
    __slots__ = ("id", "event", "data")

    def __init__(self, id, event, data):
        self.id = id
        self.event = event
        self.data = data


class SSEDecoder:
    def __init__(self):
        self._event = ""
        self._data = []
        self._last_event_id = ""

    def decode(self, line):
        if not line:
            if not self._event and not self._data and not self._last_event_id:
                return None

            sse = ServerSentEvent(
                event=self._event,
                data="\n".join(self._data),
                id=self._last_event_id,
            )

            self._event = ""
            self._data = []
            return sse

        if line.startswith(":"):
            return None

        fieldname, _, value = line.partition(":")

        if value.startswith(" "):
            value = value[1:]

        if fieldname == "event":
            self._event = value
        elif fieldname == "data":
            self._data.append(value)
        elif fieldname == "id":
            if "\0" in value:
                pass
            else:
                self._last_event_id = value
        else:
            pass
        return None


def fetch(method, url, *, params=None, headers=None, body=None, timeout=None):
    return Request(method, url, params, headers, body, timeout)


### Legacy API


def _validate_status(resp):
    if not 200 <= resp.status < 300:
        raise RuntimeError(f"http status check failed, status={resp.status}")


def _read(resp, typ):
    if typ == "json":
        return resp.json()
    elif typ == "text":
        return resp.text()
    else:
        return resp.read()


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
        return _read(resp, response)


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
        return _read(resp, response)


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
            typ = extra.get("type", "json")
            if typ == "json":
                return await resp.ajson()
            elif typ == "text":
                return await resp.atext()
            else:
                return await resp.aread()
    except Exception as e:
        if ignore_error:
            return e
        raise e


async def _fetch_all(requests, ignore_error):
    return await asyncio.gather(*[_fetch(r, ignore_error) for r in requests])


def fetch_all(requests, ignore_error=False):
    with asyncio.Runner() as runner:
        return runner.run(_fetch_all(requests, ignore_error))
