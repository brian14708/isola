from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Iterator, Mapping
from contextlib import suppress
from typing import cast, override

import _isola_http as _http
import httpx2
import httpx2._client as _httpx2_client  # pyright: ignore[reportPrivateUsage]

from sandbox.asyncio import subscribe

_CHUNK_SIZE = 16 * 1024


def _timeout(request: httpx2.Request) -> float | None:
    timeout = request.extensions.get("timeout")
    if not isinstance(timeout, Mapping):
        return None

    # The native bridge has one timeout for the exchange. HTTPX2's read timeout
    # is the closest equivalent and is also what users set via timeout=<float>.
    timeout_values = cast("Mapping[str, object]", timeout)
    read_timeout = timeout_values.get("read")
    return float(read_timeout) if isinstance(read_timeout, int | float) else None


def _response(
    resp: _http.Response, stream: httpx2.SyncByteStream | httpx2.AsyncByteStream
) -> httpx2.Response:
    return httpx2.Response(
        status_code=resp.status(),
        headers=resp.headers(),
        stream=stream,
        extensions={"http_version": b"HTTP/1.1"},
    )


class _ResponseStream(httpx2.SyncByteStream):
    def __init__(self, resp: _http.Response) -> None:
        self._resp = resp

    @override
    def __iter__(self) -> Iterator[bytes]:
        buf = _http.new_buffer("bytes")
        while (poll := self._resp.read_into(buf, _CHUNK_SIZE)) is not None:
            while (data := buf.next()) is not None:
                yield cast("bytes", data)
            poll.wait()
        while (data := buf.next()) is not None:
            yield cast("bytes", data)

    @override
    def close(self) -> None:
        self._resp.close()


class _AsyncResponseStream(httpx2.AsyncByteStream):
    def __init__(self, resp: _http.Response) -> None:
        self._resp = resp

    @override
    async def __aiter__(self) -> AsyncIterator[bytes]:
        buf = _http.new_buffer("bytes")
        while (poll := self._resp.read_into(buf, _CHUNK_SIZE)) is not None:
            while (data := buf.next()) is not None:
                yield cast("bytes", data)
            await subscribe(poll)
        while (data := buf.next()) is not None:
            yield cast("bytes", data)

    @override
    async def aclose(self) -> None:
        self._resp.close()


class IsolaTransport(httpx2.BaseTransport):
    # Egress proxy and TLS are host policy, not guest configuration: the guest
    # has no sockets, so swallow the transport kwargs httpx2 passes and let the
    # host apply its own policy to every request.
    def __init__(self, **_: object) -> None:
        pass

    @override
    def handle_request(self, request: httpx2.Request) -> httpx2.Response:
        upload = _http.open_upload()
        try:
            pending = _http.fetch_stream(
                request.method,
                str(request.url),
                None,
                dict(request.headers.items()),
                upload,
                _timeout(request),
            )
            for chunk in cast("httpx2.SyncByteStream", request.stream):
                data = bytes(chunk)
                write = _http.write_upload(upload, data)
                write.wait()
                write.get()
            _http.close_upload(upload)
            resp = pending.wait()
        except Exception as error:
            with suppress(Exception):
                _http.close_upload(upload)
            message = str(error)
            if "timed out" in message.lower():
                raise httpx2.ReadTimeout(message, request=request) from error
            raise httpx2.TransportError(message, request=request) from error
        return _response(resp, _ResponseStream(resp))


class IsolaAsyncTransport(httpx2.AsyncBaseTransport):
    def __init__(self, **_: object) -> None:
        pass

    @override
    async def handle_async_request(self, request: httpx2.Request) -> httpx2.Response:
        upload = _http.open_upload()

        async def _pump_upload() -> None:
            try:
                async for chunk in cast("httpx2.AsyncByteStream", request.stream):
                    data = bytes(chunk)
                    await subscribe(_http.write_upload(upload, data))
            finally:
                with suppress(Exception):
                    _http.close_upload(upload)

        producer = asyncio.create_task(_pump_upload())
        try:
            pending = _http.fetch_stream(
                request.method,
                str(request.url),
                None,
                dict(request.headers.items()),
                upload,
                _timeout(request),
            )
            resp = await subscribe(pending)
            await producer
        except Exception as error:
            producer.cancel()
            with suppress(asyncio.CancelledError):
                await producer
            with suppress(Exception):
                _http.close_upload(upload)
            message = str(error)
            if "timed out" in message.lower():
                raise httpx2.ReadTimeout(message, request=request) from error
            raise httpx2.TransportError(message, request=request) from error
        return _response(resp, _AsyncResponseStream(resp))


def install() -> None:
    """Route HTTPX2's default sync and async transports through Isola."""
    transports = (
        (httpx2, "HTTPTransport", IsolaTransport),
        (httpx2, "AsyncHTTPTransport", IsolaAsyncTransport),
        (_httpx2_client, "HTTPTransport", IsolaTransport),
        (_httpx2_client, "AsyncHTTPTransport", IsolaAsyncTransport),
    )
    for module, name, transport in transports:
        setattr(module, name, transport)
