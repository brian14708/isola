from __future__ import annotations

import asyncio
import contextlib
from collections import deque
from typing import TYPE_CHECKING, Unpack, cast, overload, override

if TYPE_CHECKING:
    from collections.abc import (
        AsyncGenerator,
        Awaitable,
        Callable,
        Coroutine,
        Generator,
    )
    from contextvars import Context

    import _isola_sys

    type _Coroutine[T] = Coroutine[object, object, T]

__all__ = [
    "hostcall",
    "run",
    "subscribe",
]

with contextlib.suppress(ImportError):
    import _isola_sys


async def subscribe[T](
    fut: _isola_sys.Pollable[T],
) -> T:
    loop = asyncio.get_running_loop()
    assert isinstance(loop, PollLoop), (
        "subscribe() must be called from a PollLoop context"
    )
    return await loop.subscribe(fut)


class PollLoop(asyncio.AbstractEventLoop):
    __slots__: tuple[str, ...] = ("closed", "handles", "running", "wakers")

    def __init__(self) -> None:
        self.wakers: list[
            tuple[_isola_sys.Pollable[object], asyncio.Future[object] | asyncio.Handle]
        ] = []
        self.running: bool = False
        self.closed: bool = False
        self.handles: deque[asyncio.Handle] = deque()

    def subscribe[T](self, pollable: _isola_sys.Pollable[T]) -> asyncio.Future[T]:
        waker = self.create_future()
        self.wakers.append((pollable, waker))
        return cast("asyncio.Future[T]", waker)

    @override
    def run_until_complete[T](self, future: Awaitable[T]) -> T:
        try:
            self.running = True
            asyncio.events._set_running_loop(self)  # noqa: SLF001
            return self._run_until_complete(future)
        finally:
            self._cleanup()
            self.running = False
            asyncio.events._set_running_loop(None)  # noqa: SLF001

    def run_async_generator[T](self, generator: AsyncGenerator[T]) -> Generator[T]:
        it = aiter(generator)
        try:
            self.running = True
            asyncio.events._set_running_loop(self)  # noqa: SLF001

            while True:
                try:
                    yield self._run_until_complete(anext(it))
                except StopAsyncIteration:
                    break
        finally:
            self._cleanup()
            self.running = False
            asyncio.events._set_running_loop(None)  # noqa: SLF001

    def _run_until_complete[T](self, future: Awaitable[T]) -> T:
        future = asyncio.ensure_future(future, loop=self)
        while self.running and (self.handles or self.wakers) and (not future.done()):
            while self.handles:
                handle = self.handles.popleft()
                if not handle._cancelled:  # noqa: SLF001
                    handle._run()  # noqa: SLF001

            if self.wakers and (readyset := _isola_sys.poll(self.wakers)):
                new_wakers: list[
                    tuple[
                        _isola_sys.Pollable[object],
                        asyncio.Future[object] | asyncio.Handle,
                    ]
                ] = []
                for is_ready, (pollable, waker) in zip(
                    readyset, self.wakers, strict=True
                ):
                    if is_ready:
                        if isinstance(waker, asyncio.Handle):
                            self.handles.append(waker)
                        elif not waker.cancelled():
                            waker.set_result(pollable.get())
                            pollable.release()
                    else:
                        new_wakers.append((pollable, waker))
                self.wakers = new_wakers

        if not future.done() and self.running:
            msg = "Deadlock detected"
            raise RuntimeError(msg)
        return future.result()

    def _cleanup(self) -> None:
        while self.handles or self.wakers:
            while self.handles:
                handle = self.handles.popleft()
                if not handle._cancelled:  # noqa: SLF001
                    handle._run()  # noqa: SLF001

            for pollable, waker in self.wakers:
                _ = waker.cancel()
                pollable.release()
            self.wakers.clear()

    @override
    def is_running(self) -> bool:
        return self.running

    @override
    def is_closed(self) -> bool:
        return self.closed

    @override
    def stop(self) -> None:
        self.running = False

    @override
    def close(self) -> None:
        self.running = False
        self.closed = True

    @override
    def call_exception_handler(self, context: dict[str, object]) -> None:
        pass

    @override
    def call_soon[*Ts](
        self,
        callback: Callable[[Unpack[Ts]], object],
        *args: *Ts,
        context: Context | None = None,
    ) -> asyncio.Handle:
        handle = asyncio.Handle(callback, args, self, context)
        self.handles.append(handle)
        return handle

    @override
    def call_later[*Ts](
        self,
        delay: float,
        callback: Callable[[Unpack[Ts]], object],
        *args: *Ts,
        context: Context | None = None,
    ) -> asyncio.TimerHandle:
        handle = asyncio.TimerHandle(delay + self.time(), callback, args, self, context)
        fut = _isola_sys.sleep(delay)
        self.wakers.append((fut, handle))
        return handle

    @override
    def call_at[*Ts](
        self,
        when: float,
        callback: Callable[[Unpack[Ts]], object],
        *args: *Ts,
        context: Context | None = None,
    ) -> asyncio.TimerHandle:
        return self.call_later(when - self.time(), callback, *args, context=context)

    def _timer_handle_cancelled(self, handle: asyncio.TimerHandle) -> None:
        w = self.wakers
        ln = len(w)
        for i in range(ln):
            pollable, waker = w[i]
            if waker is handle:
                pollable.release()
                w[i] = w[ln - 1]
                _ = w.pop()
                return

    @override
    def time(self) -> float:
        return _isola_sys.monotonic()

    @override
    def create_task[T](
        self,
        coro: _Coroutine[T],
        *,
        name: str | None = None,
        context: Context | None = None,
    ) -> asyncio.Task[T]:
        return asyncio.Task(coro, loop=self, name=name, context=context)

    @override
    def create_future(self) -> asyncio.Future[object]:
        return asyncio.Future(loop=self)

    @override
    def get_debug(self) -> bool:
        return False

    @override
    async def shutdown_asyncgens(self) -> None:
        pass

    @override
    async def shutdown_default_executor(self, timeout: float | None = None) -> None:  # noqa: ASYNC109
        pass


def _iter[T](it: AsyncGenerator[T]) -> Generator[T]:
    with asyncio.Runner(loop_factory=PollLoop) as runner:
        loop = runner.get_loop()
        assert isinstance(loop, PollLoop), "runner.get_loop() must return a PollLoop"
        yield from loop.run_async_generator(it)


@overload
def run[T](main: _Coroutine[T]) -> T: ...
@overload
def run[T](main: AsyncGenerator[T]) -> Generator[T]: ...
def run[T](main: _Coroutine[T] | AsyncGenerator[T]) -> T | Generator[T]:
    if hasattr(main, "__aiter__"):
        return _iter(cast("AsyncGenerator[T]", main))
    with asyncio.Runner(loop_factory=PollLoop) as runner:
        return runner.run(cast("Coroutine[None, None, T]", main))


async def _aiter_arg(args: _isola_sys.ArgIter) -> AsyncGenerator[object]:  # pyright:ignore[reportUnusedFunction]
    while True:
        ok, result, poll = args.read()
        if not ok:
            break
        elif poll is not None:
            await subscribe(poll)
        else:
            yield result


async def hostcall(call_type: str, payload: object) -> object:
    future_hostcall = _isola_sys.hostcall(call_type, payload)
    return await subscribe(future_hostcall)
