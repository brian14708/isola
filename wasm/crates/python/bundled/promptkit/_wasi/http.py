import asyncio
import binascii
import os
from typing import IO, TYPE_CHECKING, Literal, cast, final, overload

import _promptkit_http as _http
import _promptkit_rpc

from promptkit.asyncio import run as asyncio_run
from promptkit.asyncio import subscribe

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Generator

    import _promptkit_sys

type _FileType = bytes | IO[bytes] | tuple[str, bytes | IO[bytes], str]
type _MethodType = Literal["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"]
type _ResponseType = Literal["json", "text", "bytes"]
type _IterResponseType = Literal["lines", "bytes", "sse"]


@final
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

    def __init__(
        self,
        method: _MethodType,
        url: str,
        params: dict[str, str] | None,
        headers: dict[str, str] | None,
        body: object | bytes | None,
        timeout: float | None,
        extra: dict[str, object] | None = None,
    ):
        self.method = method
        self.url = url
        self.params = params
        self.headers = headers
        self.body = body
        self.timeout = timeout
        self.extra = extra
        self.resp: Response | None = None

    def _fetch(self) -> "_promptkit_sys.Pollable[_http.Response]":
        req = _http.fetch(
            self.method, self.url, self.params, self.headers, self.body, self.timeout
        )
        self.body = None
        return req

    async def __aenter__(self) -> "Response":
        self.resp = Response(await subscribe(self._fetch()))
        return self.resp

    async def __aexit__(self, *_: object) -> None:
        if self.resp:
            self.resp.close()

    def __enter__(self) -> "Response":
        self.resp = Response(self._fetch().wait())
        return self.resp

    def __exit__(self, *_: object) -> None:
        if self.resp:
            self.resp.close()


@final
class ServerSentEvent:
    __slots__ = ("id", "event", "data")

    def __init__(self, id: str | None, event: str | None, data: str):
        self.id = id
        self.event = event
        self.data = data


@final
class Response:
    __slots__ = ("resp", "_status", "_headers")

    def __init__(self, resp: "_http.Response"):
        self.resp: _http.Response | None = resp
        self._status: int | None = None
        self._headers: dict[str, str] | None = None

    @property
    def status(self) -> int:
        if self.resp is None:
            raise RuntimeError("Response is closed")
        if self._status is None:
            self._status = self.resp.status()
        return self._status

    @property
    def headers(self) -> dict[str, str]:
        if self.resp is None:
            raise RuntimeError("Response is closed")
        if self._headers is None:
            self._headers = self.resp.headers()
        return self._headers

    def close(self) -> None:
        if self.resp:
            self.resp.close()
            self.resp = None

    # async read methods

    @overload
    async def _aread(self, encoding: Literal["json"], size: int) -> object: ...
    @overload
    async def _aread(self, encoding: Literal["text"], size: int) -> str: ...
    @overload
    async def _aread(self, encoding: Literal["bytes"], size: int) -> bytes: ...
    async def _aread(self, encoding: _ResponseType, size: int) -> object | str | bytes:
        if self.resp is None:
            raise RuntimeError("Response is closed")
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, size)) is not None:
            await subscribe(poll)
        return buf.read_all()

    async def ajson(self) -> object:
        return await self._aread("json", -1)

    async def atext(self) -> str:
        return await self._aread("text", -1)

    async def aread(self, size: int = -1) -> bytes:
        return await self._aread("bytes", size)

    # async iterator methods

    async def _aiter(self, encoding: _IterResponseType) -> "AsyncGenerator[object]":
        if self.resp is None:
            raise RuntimeError("Response is closed")
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, 16384)) is not None:
            while (data := buf.next()) is not None:
                yield data
            await subscribe(poll)
        while (data := buf.next()) is not None:
            yield data

    async def aiter_bytes(self) -> "AsyncGenerator[bytes]":
        async for data in cast("AsyncGenerator[bytes]", self._aiter("bytes")):
            yield data

    async def aiter_lines(self) -> "AsyncGenerator[str]":
        async for line in cast("AsyncGenerator[str]", self._aiter("lines")):
            yield line

    async def aiter_sse(self) -> "AsyncGenerator[ServerSentEvent]":
        async for id, event, data in cast(
            "AsyncGenerator[tuple[str,str,str]]", self._aiter("sse")
        ):
            yield ServerSentEvent(id, event, data)

    # sync read methods

    @overload
    def _read(self, encoding: Literal["json"], size: int) -> object: ...
    @overload
    def _read(self, encoding: Literal["text"], size: int) -> str: ...
    @overload
    def _read(self, encoding: Literal["bytes"], size: int) -> bytes: ...
    def _read(self, encoding: _ResponseType, size: int = -1) -> object | str | bytes:
        if self.resp is None:
            raise RuntimeError("Response is closed")
        return self.resp.blocking_read(encoding, size)

    def read(self, size: int = -1) -> bytes:
        return self._read("bytes", size)

    def json(self) -> object:
        return self._read("json", -1)

    def text(self) -> str:
        return self._read("text", -1)

    # sync iterator methods

    def _iter(self, encoding: _IterResponseType) -> "Generator[object]":
        if self.resp is None:
            raise RuntimeError("Response is closed")
        buf = _http.new_buffer(encoding)
        while (poll := self.resp.read_into(buf, 16384)) is not None:
            while (data := buf.next()) is not None:
                yield data
            poll.wait()
        while (data := buf.next()) is not None:
            yield data

    def iter_bytes(self) -> "Generator[bytes]":
        return cast("Generator[bytes]", self._iter("bytes"))

    def iter_lines(self) -> "Generator[str]":
        return cast("Generator[str]", self._iter("lines"))

    def iter_sse(self) -> "Generator[ServerSentEvent]":
        for id, event, data in cast("Generator[tuple[str,str,str]]", self._iter("sse")):
            yield ServerSentEvent(id, event, data)


@final
class WebSocketRequest:
    __slots__ = ("url", "headers", "conn", "timeout")

    def __init__(self, url: str, headers: dict[str, str] | None, timeout: float | None):
        self.url = url
        self.headers = headers
        self.timeout = timeout
        self.conn: Websocket | None = None

    def _conn(self) -> "_promptkit_sys.Pollable[_promptkit_rpc.Connection]":
        return _promptkit_rpc.connect(self.url, self.headers, self.timeout)

    async def __aenter__(self) -> "Websocket":
        self.conn = Websocket(await subscribe(self._conn()))
        return self.conn

    async def __aexit__(self, *_: object) -> None:
        if self.conn:
            self.conn.shutdown()

    def __enter__(self) -> "Websocket":
        self.conn = Websocket(self._conn().wait())
        return self.conn

    def __exit__(self, *_: object) -> None:
        if self.conn:
            self.conn.shutdown()


@final
class Websocket:
    __slots__ = ("conn",)

    def __init__(self, conn: "_promptkit_rpc.Connection"):
        self.conn = conn

    def shutdown(self) -> None:
        self.conn.shutdown()

    def close(self) -> None:
        self.conn.close()

    async def arecv(self) -> bytes | str | None:
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                await subscribe(poll)
            else:
                return value

    async def arecv_streaming(self) -> "AsyncGenerator[bytes | str]":
        while True:
            value = await self.arecv()
            if value is None:
                return
            yield value

    def recv(self) -> bytes | str | None:
        while True:
            ok, value, poll = self.conn.recv()
            if not ok:
                return None
            if poll is not None:
                poll.wait()
            else:
                return value

    def recv_streaming(self) -> "Generator[bytes | str]":
        while (value := self.recv()) is not None:
            yield value

    async def asend(self, value: bytes | str) -> None:
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                await subscribe(poll)
            else:
                break

    def send(self, value: bytes | str) -> None:
        while True:
            poll = self.conn.send(value)
            if poll is not None:
                poll.wait()
            else:
                break


def fetch(
    method: _MethodType,
    url: str,
    *,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    files: dict[str, _FileType] | None = None,
    body: object | bytes | None = None,
    timeout: float | None = None,
    proxy: str | None = None,
) -> Request:
    if files:
        if body:
            raise ValueError("Cannot specify both files and body")
        body, ty = _encode_multipart_formdata(files)
        if not headers:
            headers = {}
        headers["Content-Type"] = ty
    if proxy:
        if not headers:
            headers = {}
        headers["x-promptkit-proxy"] = proxy
    return Request(method, url, params, headers, body, timeout)


def ws_connect(
    url: str, *, headers: dict[str, str] | None = None, timeout: float | None = None
) -> WebSocketRequest:
    return WebSocketRequest(url, headers, timeout)


def _encode_multipart_formdata(fields: dict[str, _FileType]) -> tuple[bytes, str]:
    b_boundary = binascii.hexlify(os.urandom(16))
    boundary = b_boundary.decode()
    b_boundary = b"--" + b_boundary
    parts: list[bytes] = []

    for field, value in fields.items():
        if isinstance(value, tuple):
            filename, fileobj, mime = value
        else:
            filename, fileobj, mime = field, value, "application/octet-stream"

        content_disposition = (
            f'Content-Disposition: form-data; name="{field}"; filename="{filename}"'
        )
        parts.extend(
            (
                b_boundary,
                content_disposition.encode(),
                f"Content-Type: {mime}".encode(),
                b"",
                fileobj if isinstance(fileobj, bytes) else fileobj.read(),
            )
        )

    parts.extend((b_boundary + b"--", b""))
    body = b"\r\n".join(parts)
    return body, f"multipart/form-data; boundary={boundary}"


### Legacy API


def _validate_status(resp: Response) -> None:
    if not 200 <= resp.status < 300:
        try:
            content = resp.read(size=1024 * 128).decode("utf-8", "replace")
        except Exception:
            content = "<unable to read response content>"
        raise RuntimeError(
            f"http status check failed, status={resp.status}, content={repr(content)}"
        )


def get(
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,
) -> object:
    with Request("GET", url, params, headers, None, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return resp._read(response, -1)


def get_async(
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,
) -> Request:
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
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
) -> "Generator[object]":
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
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,
) -> object:
    with Request("POST", url, None, headers, data, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return resp._read(response, -1)


def post_async(
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,
) -> Request:
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


def post_sse(
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
) -> "Generator[object]":
    with Request("POST", url, None, headers, data, timeout) as resp:
        _validate_status(resp)
        for event in resp.iter_sse():
            if event.data.startswith("[DONE]"):
                break
            yield _http.loads_json(event.data)


async def _fetch(r: Request, ignore_error: bool) -> object | bytes | str | Exception:
    extra = r.extra or {}
    try:
        async with r as resp:
            if extra.get("validate"):
                _validate_status(resp)
            return await resp._aread(
                cast("_ResponseType", extra.get("type", "json")), -1
            )
    except Exception as e:
        if ignore_error:
            return e
        raise e


async def _fetch_all(
    requests: list[Request], ignore_error: bool
) -> list[object | Exception]:
    return await asyncio.gather(*[_fetch(r, ignore_error) for r in requests])


def fetch_all(
    requests: list[Request], ignore_error: bool = False
) -> list[object | Exception]:
    return asyncio_run(_fetch_all(requests, ignore_error))
