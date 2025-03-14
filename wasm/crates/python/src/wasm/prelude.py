from promptkit.asyncio import WasiEventLoopPolicy
import asyncio

asyncio.set_event_loop_policy(WasiEventLoopPolicy())
