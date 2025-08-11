import asyncio
import time

from promptkit.http import ws_connect


def simple_echo(ws_url: str) -> None:
    """Test basic WebSocket echo functionality."""
    message = f"hello-{time.time()}"

    with ws_connect(ws_url + "/echo") as ws:
        ws.send(message)
        response = ws.recv()
        assert response == message


async def async_echo(ws_url: str) -> None:
    """Test async WebSocket echo functionality."""
    message = f"async-hello-{time.time()}"

    async with ws_connect(ws_url + "/echo") as ws:
        await ws.asend(message)
        response = await ws.arecv()
        assert response == message


def binary_echo(ws_url: str) -> None:
    """Test WebSocket binary message handling."""
    binary_data = b"binary-test-" + str(time.time()).encode()

    with ws_connect(ws_url + "/echo") as ws:
        ws.send(binary_data)
        response = ws.recv()
        assert response == binary_data


def connection_close(ws_url: str) -> None:
    """Test proper WebSocket connection closing."""
    with ws_connect(ws_url + "/echo") as ws:
        ws.close()
        response = ws.recv()
        assert response is None


def connection_close_error_code(ws_url: str) -> None:
    """Test proper WebSocket connection closing."""
    with ws_connect(ws_url + "/echo") as ws:
        ws.close(1011, "test close")
        try:
            _ = ws.recv()
        except Exception as e:
            assert "1011" in str(e)
            assert "test close" in str(e)


async def timeout_test(ws_url: str) -> None:
    """Test WebSocket connection timeout."""
    # Test with a very short timeout to ensure timeout behavior
    try:
        async with ws_connect(ws_url + "/slow", timeout=0.01) as ws:
            await ws.asend("test")
            await ws.arecv()
    except Exception as e:
        # Expect some kind of timeout or connection error
        assert "timeout" in str(e).lower() or "connection" in str(e).lower()


async def concurrent_connections(ws_url: str) -> None:
    """Test multiple concurrent WebSocket connections."""

    async def single_connection_task(connection_id: int) -> str:
        """Single connection task that sends and receives a message."""
        message = f"concurrent-{connection_id}-{time.time()}"
        async with ws_connect(ws_url + "/echo") as ws:
            await ws.asend(message)
            response = await ws.arecv()
            assert response == message
            return message

    # Create multiple concurrent tasks
    tasks = [single_connection_task(i) for i in range(5)]

    # Run all tasks concurrently
    results = await asyncio.gather(*tasks)

    # Verify all tasks completed successfully
    assert len(results) == 5
    assert all(result.startswith("concurrent-") for result in results)


async def concurrent_messages(ws_url: str) -> None:
    """Test concurrent sending and streaming receiving with separate tasks."""

    messages = [f"msg-{i}-{time.time()}" for i in range(3)]

    async with ws_connect(ws_url + "/echo") as ws:

        async def sender_task() -> None:
            """Task that sends messages and closes connection."""
            for msg in messages:
                await ws.asend(msg)

        async def receiver_task(expected_count: int) -> list[bytes | str]:
            """Task that receives messages using streaming."""
            received = []
            async for msg in ws.arecv_streaming():
                received.append(msg)
                if len(received) == expected_count:
                    break
            return received

        _s1, _s2, _s3, received_messages = await asyncio.gather(
            sender_task(),
            sender_task(),
            sender_task(),
            receiver_task(3 * len(messages)),
        )
        assert set(received_messages) == set(messages)
