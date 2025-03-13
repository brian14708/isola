import asyncio
import contextlib
import urllib.parse
import json
from promptkit.http import fetch

__all__ = ["connect"]


class Session:
    def __init__(self, url, headers):
        self.queue = asyncio.Queue(8)
        self.url = url
        self.headers = headers
        self.id = 1

    async def initialize(self, *, capabilities={}):
        endpoint = await self._recv("endpoint")
        self.url = urllib.parse.urljoin(self.url, endpoint)

        await self.send_request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": capabilities,
                "clientInfo": {
                    "name": "PromptKit",
                    "version": "1.0.0",
                },
            },
        )
        await self.send_notification("notifications/initialized")

    async def send_request(self, method, params=None):
        id = self.id
        self.id += 1
        request = {
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        }
        if params:
            request["params"] = params
        async with fetch("POST", self.url, headers=self.headers, body=request) as resp:
            if not 200 <= resp.status < 300:
                raise RuntimeError(f"http status check failed, status={resp.status}")

        msg = await self._recv("message")
        data = json.loads(msg)
        if data["id"] != id:
            raise RuntimeError("invalid id")
        if "error" in data:
            raise RuntimeError(data["error"])
        return data["result"]

    async def send_notification(self, method, params=None):
        request = {
            "jsonrpc": "2.0",
            "method": method,
        }
        if params:
            request["params"] = params
        async with fetch("POST", self.url, headers=self.headers, body=request) as resp:
            if not 200 <= resp.status < 300:
                raise RuntimeError(f"http status check failed, status={resp.status}")

    async def _recv(self, event):
        while True:
            t = await self.queue.get()
            if t.event == event:
                return t.data

    async def _on_recv(self, iter):
        async for event in iter:
            await self.queue.put(event)
        self.queue.shutdown()


@contextlib.asynccontextmanager
async def sse_connect(url, *, headers=None, timeout=None):
    async with fetch("GET", url, headers=headers, timeout=timeout) as resp:
        if not 200 <= resp.status < 300:
            raise RuntimeError(f"http status check failed, status={resp.status}")

        session = Session(url, headers)
        task = asyncio.create_task(session._on_recv(resp.aiter_sse()))
        try:
            yield session
        finally:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
