import asyncio
import time

import _promptkit_sys

__all__ = [
    "run",
    "subscribe",
]


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
        self.closed = False
        self.handles = []

    def run_until_complete(self, future):
        try:
            self.running = True
            asyncio.events._set_running_loop(self)
            result = self._run_until_complete(future).result()
            return result
        finally:
            self._cleanup()
            self.running = False
            asyncio.events._set_running_loop(None)

    def run_async_generator(self, generator):
        it = aiter(generator)
        try:
            self.running = True
            asyncio.events._set_running_loop(self)

            while True:
                future = self._run_until_complete(anext(it))
                exc = future.exception()
                if exc is None:
                    yield future.result()
                elif isinstance(exc, StopAsyncIteration):
                    return
                else:
                    raise exc
        finally:
            self._cleanup()
            self.running = False
            asyncio.events._set_running_loop(None)

    def _run_until_complete(self, future):
        future = asyncio.ensure_future(future, loop=self)
        while self.running and (self.handles or self.wakers) and (not future.done()):
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

                for ready, pollable, waker in zip(ready, pollables, wakers):
                    if ready:
                        if isinstance(waker, asyncio.Handle):
                            self.handles.append(waker)
                        elif not waker.cancelled():
                            waker.set_result(None)
                        pollable.release()
                    else:
                        new_wakers.append((pollable, waker))

                self.wakers = new_wakers
        return future

    def _cleanup(self):
        while self.handles or self.wakers:
            handles = self.handles
            self.handles = []
            for handle in handles:
                if not handle._cancelled:
                    handle._run()

            wakers = self.wakers
            self.wakers = []
            for pollable, waker in wakers:
                waker.cancel()
                pollable.release()

    def is_running(self):
        return self.running

    def is_closed(self):
        return self.closed

    def stop(self):
        self.running = False

    def close(self):
        self.running = False
        self.closed = True

    def call_exception_handler(self, context):
        pass

    def call_soon(self, callback, *args, context=None):
        handle = asyncio.Handle(callback, args, self, context)
        self.handles.append(handle)
        return handle

    def call_later(self, delay, callback, *args, context=None):
        handle = asyncio.TimerHandle(delay + self.time(), callback, args, self, context)
        fut = _promptkit_sys.sleep(delay)
        self.wakers.append((fut, handle))
        return handle

    def call_at(self, when, callback, *args, context=None):
        return self.call_later(when - self.time(), callback, *args, context=context)

    def _timer_handle_cancelled(self, handle):
        for i, (pollable, waker) in enumerate(self.wakers):
            if waker == handle:
                self.wakers.pop(i)
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

    async def shutdown_asyncgens(self):
        pass

    async def shutdown_default_executor(self, timeout=None):
        pass


def _iter(runner, it):
    try:
        loop = runner.get_loop()
        for v in loop.run_async_generator(it):
            yield v
    finally:
        runner.close()


def run(main):
    runner = asyncio.Runner(loop_factory=PollLoop)
    if hasattr(main, "__aiter__"):
        return _iter(runner, main)
    else:
        with runner:
            return runner.run(main)


async def _aiter_arg(args):
    while (result := args.read()) is not None:
        if type(result) is _promptkit_sys.PyPollable:
            await subscribe(result)
        else:
            yield result
