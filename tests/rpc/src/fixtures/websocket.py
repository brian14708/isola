"""WebSocket test server for pytest fixtures."""

import asyncio

import websockets
import websockets.asyncio.server


class WebSocketTestServer:
    def __init__(self, host: str = "localhost", port: int = 0) -> None:
        self.host = host
        self.port = port
        self.server: websockets.asyncio.server.Server | None = None
        self.url: str | None = None

    @staticmethod
    async def echo_handler(
        websocket: websockets.asyncio.server.ServerConnection,
    ) -> None:
        """Echo server that returns received messages."""
        try:
            path = websocket.request.path
            if path == "/echo":
                async for message in websocket:
                    await websocket.send(message)
            elif path == "/slow":
                # Simulate slow response for timeout testing
                await asyncio.sleep(0.2)
                async for message in websocket:
                    await websocket.send(message)
        except websockets.exceptions.ConnectionClosed:
            pass

    async def start_server(self) -> None:
        # Use modern websockets API
        self.server = await websockets.asyncio.server.serve(
            self.echo_handler, self.host, self.port
        )
        # Get the actual port if 0 was specified
        if self.server is not None:
            actual_port = next(iter(self.server.sockets)).getsockname()[1]
            self.url = f"ws://{self.host}:{actual_port}"

    async def stop_server(self) -> None:
        if self.server:
            self.server.close()
            await self.server.wait_closed()
