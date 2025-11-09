# pyright: basic

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, Literal, NamedTuple, cast

import httpx

if TYPE_CHECKING:
    from collections.abc import AsyncGenerator, Generator

type _FileType = bytes | tuple[str, bytes, str]
type _MethodType = Literal["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD"]
type _ResponseType = Literal["json", "text", "bytes"]


class Request:
    __slots__ = ("client", "extra", "request", "response", "timeout")

    def __init__(
        self,
        method: _MethodType,
        url: str,
        params: dict[str, str] | None,
        headers: dict[str, str] | None,
        body: object | bytes | None,
        timeout: float | None,
        files: dict[str, _FileType] | None = None,
        extra: dict[str, object] | None = None,
    ) -> None:
        self.client: httpx.AsyncClient | httpx.Client | None = None
        self.response: Response | None = None

        # Build httpx.Request with proper types
        if isinstance(body, bytes):
            self.request = httpx.Request(
                method,
                url,
                params=params,
                headers=headers,
                content=body,
                files=files,
            )
        elif body is not None:
            self.request = httpx.Request(
                method,
                url,
                params=params,
                headers=headers,
                json=body,
                files=files,
            )
        else:
            self.request = httpx.Request(
                method,
                url,
                params=params,
                headers=headers,
                files=files,
            )
        if timeout is None:
            self.timeout = httpx.Timeout(600)
        else:
            self.timeout = httpx.Timeout(timeout)
        self.extra = extra

    async def __aenter__(self) -> Response:
        self.client = httpx.AsyncClient(timeout=self.timeout)
        self.response = Response(await self.client.send(self.request, stream=True))
        return self.response

    async def __aexit__(self, _type: object, _value: object, _trace: object) -> None:
        if self.response is not None:
            await self.response.aclose()
        if self.client is not None:
            await cast("httpx.AsyncClient", self.client).aclose()

    def __enter__(self) -> Response:
        self.client = httpx.Client(timeout=self.timeout)
        self.response = Response(self.client.send(self.request, stream=True))
        return self.response

    def __exit__(self, _type: object, _value: object, _trace: object) -> None:
        if self.response is not None:
            self.response.close()
        if self.client is not None:
            cast("httpx.Client", self.client).close()


class Response:
    def __init__(self, response: httpx.Response) -> None:
        self.response: httpx.Response = response
        self.consumed: bool = False

    def close(self) -> None:
        self.response.close()

    async def aclose(self) -> None:
        await self.response.aclose()

    @property
    def status(self) -> int:
        return self.response.status_code

    @property
    def headers(self) -> httpx.Headers:
        return self.response.headers

    async def aread(self) -> bytes:
        if self.consumed:
            msg = "Response already read"
            raise RuntimeError(msg)
        content = await self.response.aread()
        self.consumed = True
        return content

    def read(self) -> bytes:
        if self.consumed:
            msg = "Response already read"
            raise RuntimeError(msg)
        content = self.response.read()
        self.consumed = True
        return content

    async def atext(self) -> str:
        return (await self.aread()).decode()

    def text(self) -> str:
        return self.read().decode()

    async def ajson(self) -> object:
        return json.loads(await self.aread())

    def json(self) -> object:
        return json.loads(self.read())

    async def aiter_sse(self) -> AsyncGenerator[ServerSentEvent]:
        decoder = SSEDecoder()
        async for line in self.response.aiter_lines():
            if event := decoder.decode(line):
                yield event

    def iter_sse(self) -> Generator[ServerSentEvent]:
        decoder = SSEDecoder()
        for line in self.response.iter_lines():
            if event := decoder.decode(line):
                yield event

    async def aiter_lines(self) -> AsyncGenerator[str]:
        async for line in self.response.aiter_lines():
            yield line

    def iter_lines(self) -> Generator[str]:
        yield from self.response.iter_lines()

    async def aiter_bytes(self) -> AsyncGenerator[bytes]:
        async for data in self.response.aiter_bytes():
            yield data

    def iter_bytes(self) -> Generator[bytes]:
        yield from self.response.iter_bytes()


class ServerSentEvent(NamedTuple):
    id: str | None
    event: str | None
    data: str


class SSEDecoder:
    def __init__(self) -> None:
        self._event: str = ""
        self._data: list[str] = []
        self._last_event_id: str = ""

    def decode(self, line: str) -> ServerSentEvent | None:
        if not line:
            if not self._event and not self._data and not self._last_event_id:
                return None

            sse = ServerSentEvent(
                id=self._last_event_id,
                event=self._event,
                data="\n".join(self._data),
            )

            self._event = ""
            self._data = []
            return sse

        if line.startswith(":"):
            return None

        fieldname, _, value = line.partition(":")

        value = value.removeprefix(" ")

        if fieldname == "event":
            self._event = value
        elif fieldname == "data":
            self._data.append(value)
        elif fieldname == "id":
            if "\0" in value:
                pass
            else:
                self._last_event_id = value
        return None


def fetch(
    method: _MethodType,
    url: str,
    *,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    files: dict[str, _FileType] | None = None,
    body: object | bytes | None = None,
    timeout: float | None = None,
) -> Request:
    return Request(method, url, params, headers, body, timeout, files=files)


# Legacy API


def _validate_status(resp: Response) -> None:
    if not 200 <= resp.status < 300:
        msg = f"http status check failed, status={resp.status}"
        raise RuntimeError(msg)


def _read(resp: Response, typ: _ResponseType) -> object | str | bytes:
    if typ == "json":
        return resp.json()
    if typ == "text":
        return resp.text()
    return resp.read()


def get(
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,  # noqa: FBT001, FBT002
) -> object:
    with Request("GET", url, params, headers, None, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return _read(resp, response)


def get_async(
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,  # noqa: FBT001, FBT002
) -> Request:
    return Request(
        "GET",
        url,
        params,
        headers,
        None,
        timeout,
        extra={
            "type": response,
            "validate": validate_status,
        },
    )


def get_sse(
    url: str,
    params: dict[str, str] | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
) -> Generator[object]:
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
            yield json.loads(event.data)


def post(
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,  # noqa: FBT001, FBT002
) -> object:
    with Request("POST", url, None, headers, data, timeout) as resp:
        if validate_status:
            _validate_status(resp)
        return _read(resp, response)


def post_async(
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
    response: _ResponseType = "json",
    validate_status: bool = True,  # noqa: FBT001, FBT002
) -> Request:
    return Request(
        "POST",
        url,
        None,
        headers,
        data,
        timeout,
        extra={
            "type": response,
            "validate": validate_status,
        },
    )


def post_sse(
    url: str,
    data: object | bytes | None = None,
    headers: dict[str, str] | None = None,
    timeout: float | None = None,
) -> Generator[object]:
    with Request("POST", url, None, headers, data, timeout) as resp:
        _validate_status(resp)
        for event in resp.iter_sse():
            if event.data.startswith("[DONE]"):
                break
            yield json.loads(event.data)


async def _fetch(r: Request, *, ignore_error: bool) -> object | bytes | str | Exception:
    extra = r.extra or {}
    try:
        async with r as resp:
            if r.extra and r.extra.get("validate"):
                _validate_status(resp)
            typ = extra.get("type", "json")
            if typ == "json":
                return await resp.ajson()
            if typ == "text":
                return await resp.atext()
            return await resp.aread()
    except Exception as e:
        if ignore_error:
            return e
        raise


async def _fetch_all(
    requests: list[Request], *, ignore_error: bool
) -> list[object | Exception]:
    return await asyncio.gather(*[
        _fetch(r, ignore_error=ignore_error) for r in requests
    ])


def fetch_all(
    requests: list[Request],
    ignore_error: bool = False,  # noqa: FBT001, FBT002
) -> list[object | Exception]:
    with asyncio.Runner() as runner:
        return runner.run(_fetch_all(requests, ignore_error=ignore_error))
