import asyncio
import time

import _promptkit_sys

__all__ = [
    "new_event_loop",
    "run",
    "subscribe",
]


class _TimerHandle(asyncio.TimerHandle):
    def __init__(self, waker, when, callback, args, loop, context):
        super().__init__(when, callback, args, loop)
        self.waker = waker


async def subscribe(fut):
    loop = asyncio.get_running_loop()
    waker = loop.create_future()
    try:
        loop.wakers.append((fut.subscribe(), waker))
        await waker
        return fut.get()
    finally:
        fut.release()


class PollLoop(asyncio.AbstractEventLoop):
    def __init__(self):
        self.wakers = []
        self.running = False
        self.handles = []
        self.exception = None

    def run_until_complete(self, future):
        future = asyncio.ensure_future(future, loop=self)
        self.running = True
        asyncio.events._set_running_loop(self)
        while self.running and not future.done():
            handles = self.handles
            self.handles = []
            for handle in handles:
                if not handle._cancelled:
                    handle._run()

            if self.wakers and len(handles) == 0:
                [pollables, wakers] = list(map(list, zip(*self.wakers)))

                new_wakers = []
                ready = [False] * len(pollables)
                for index in _promptkit_sys.poll(pollables):
                    ready[index] = True

                for (ready, pollable), waker in zip(zip(ready, pollables), wakers):
                    if ready:
                        if not waker.cancelled():
                            waker.set_result(None)
                        pollable.release()
                    else:
                        new_wakers.append((pollable, waker))

                self.wakers = new_wakers

            if self.exception is not None:
                raise self.exception

        return future.result()

    def is_running(self):
        return self.running

    def is_closed(self):
        return not self.running

    def stop(self):
        self.running = False

    def close(self):
        while self.handles or self.wakers:
            handles = self.handles
            self.handles = []
            for handle in handles:
                handle._run()

            for pollable, waker in self.wakers:
                waker.cancel()
                pollable.release()
            self.wakers = []

        self.running = False
        self.exception = None

    def call_exception_handler(self, context):
        self.exception = context.get("exception", None)

    def call_soon(self, callback, *args, context=None):
        handle = asyncio.Handle(callback, args, self, context)
        self.handles.append(handle)
        return handle

    def call_later(self, delay, callback, *args, context=None):
        waker = self.create_future()
        handle = _TimerHandle(waker, delay + self.time(), callback, args, self, context)
        fut = _promptkit_sys.sleep(delay)
        self.wakers.append((fut, waker))

        def cb(_):
            if not handle._cancelled:
                handle._run()

        waker.add_done_callback(cb, context=context)
        return handle

    def call_at(self, when, callback, *args, context=None):
        return self.call_later(when - self.time(), callback, *args, context=context)

    def _timer_handle_cancelled(self, handle):
        for i, (pollable, waker) in enumerate(self.wakers):
            if waker == handle.waker:
                self.wakers.pop(i)
                waker.cancel()
                pollable.release()
                break

    def time(self):
        return time.monotonic()

    def create_task(self, coro, *, name=None, context=None):
        return asyncio.Task(coro, loop=self, name=name, context=context)

    def create_future(self):
        return asyncio.Future(loop=self)

    def get_debug(self):
        return False


class LoopGuard:
    def __init__(self, loop):
        self.loop = loop

    def __enter__(self):
        asyncio.set_event_loop(self.loop)

    def __exit__(self, exc_type, exc_value, traceback):
        self.loop.close()
        asyncio.set_event_loop(None)


def new_event_loop():
    return PollLoop()


def _iter(loop, it):
    it = aiter(it)
    while True:
        try:
            yield loop.run_until_complete(anext(it))
        except StopAsyncIteration:
            break


def run(main):
    loop = new_event_loop()

    with LoopGuard(loop):
        if hasattr(main, "__aiter__"):
            return _iter(loop, main)
        return loop.run_until_complete(main)
