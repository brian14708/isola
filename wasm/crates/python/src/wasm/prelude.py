import asyncio

from promptkit.asyncio import WasiEventLoopPolicy

asyncio.set_event_loop_policy(WasiEventLoopPolicy())
