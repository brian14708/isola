import asyncio


def add(a: int, b: int, c: int = 1) -> int:
    return a + b * c


async def async_add(a: int, b: int, c: int = 1) -> int:
    async def inner() -> int:
        return a + b * c

    return await asyncio.wait_for(asyncio.create_task(inner()), 10)


def raise_exception(exc: str) -> None:
    raise RuntimeError(exc)


async def stall() -> None:
    await asyncio.sleep(3600)
