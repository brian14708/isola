import asyncio
import contextlib
from typing import (
    TYPE_CHECKING,
    Any,
    Unpack,
    cast,
    overload,
)

if TYPE_CHECKING:
    from collections.abc import (
        AsyncGenerator,
        Awaitable,
        Callable,
        Coroutine,
        Generator,
    )
    from contextvars import Context

    import _promptkit_sys

__all__ = [
    "run",
    "subscribe",
]

with contextlib.suppress(ImportError):
    import _promptkit_sys

    async def subscribe[T](
        fut: "_promptkit_sys.Pollable[T]",
    ) -> T:
        loop = asyncio.get_running_loop()
        if isinstance(loop, PollLoop):
            waker = loop.add_waker(fut)
            try:
                await waker
                return fut.get()
            finally:
                fut.release()
        else:
            raise RuntimeError("subscribe() must be called from a PollLoop context")


class PollLoop(asyncio.AbstractEventLoop):
    def __init__(self) -> None:
        self.wakers: list[
            tuple[_promptkit_sys.Pollable[Any], asyncio.Future[None] | asyncio.Handle]
        ] = []
        self.running: bool = False
        self.closed: bool = False
        self.handles: list[asyncio.Handle] = []

    def add_waker(
        self, pollable: "_promptkit_sys.Pollable[Any]"
    ) -> asyncio.Future[None]:
        waker = self.create_future()
        self.wakers.append((pollable, waker))
        return waker

    def run_until_complete[T](self, future: "Awaitable[T]") -> T:
        try:
            self.running = True
            asyncio.events._set_running_loop(self)
            return self._run_until_complete(future).result()
        finally:
            self._cleanup()
            self.running = False
            asyncio.events._set_running_loop(None)

    def run_async_generator[T](self, generator: "AsyncGenerator[T]") -> "Generator[T]":
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

    def _run_until_complete[T](self, future: "Awaitable[T]") -> asyncio.Future[T]:
        future = asyncio.ensure_future(future, loop=self)
        while self.running and (self.handles or self.wakers) and (not future.done()):
            handles = self.handles
            self.handles = []
            for handle in handles:
                if not handle._cancelled:
                    handle._run()

            if self.wakers and len(handles) == 0:
                ready_indices_set = _promptkit_sys.poll(self.wakers)

                new_wakers = []
                for i, (pollable, waker) in enumerate(self.wakers):
                    if i in ready_indices_set:
                        if isinstance(waker, asyncio.Handle):
                            self.handles.append(waker)
                        elif not waker.cancelled():
                            waker.set_result(None)
                    else:
                        new_wakers.append((pollable, waker))

                self.wakers = new_wakers
        return future

    def _cleanup(self) -> None:
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

    def is_running(self) -> bool:
        return self.running

    def is_closed(self) -> bool:
        return self.closed

    def stop(self) -> None:
        self.running = False

    def close(self) -> None:
        self.running = False
        self.closed = True

    def call_exception_handler(self, _: dict[str, Any]) -> None:
        pass

    def call_soon[*Ts](
        self,
        callback: "Callable[[Unpack[Ts]], object]",
        *args: *Ts,
        context: "Context | None" = None,
    ) -> asyncio.Handle:
        handle = asyncio.Handle(callback, args, self, context)
        self.handles.append(handle)
        return handle

    def call_later[*Ts](
        self,
        delay: float,
        callback: "Callable[[Unpack[Ts]], object]",
        *args: *Ts,
        context: "Context | None" = None,
    ) -> asyncio.TimerHandle:
        handle = asyncio.TimerHandle(delay + self.time(), callback, args, self, context)
        fut = _promptkit_sys.sleep(delay)
        self.wakers.append((fut, handle))
        return handle

    def call_at[*Ts](
        self,
        when: float,
        callback: "Callable[[Unpack[Ts]], object]",
        *args: *Ts,
        context: "Context | None" = None,
    ) -> asyncio.TimerHandle:
        return self.call_later(when - self.time(), callback, *args, context=context)

    def _timer_handle_cancelled(self, handle: asyncio.TimerHandle) -> None:
        for i, (pollable, waker) in enumerate(self.wakers):
            if waker == handle:
                self.wakers.pop(i)
                pollable.release()
                break

    def time(self) -> float:
        return _promptkit_sys.monotonic()

    def create_task[T](
        self,
        coro: "Coroutine[Any, Any, T]",
        *,
        name: str | None = None,
        context: "Context | None" = None,
    ) -> asyncio.Task[T]:
        return asyncio.Task(coro, loop=self, name=name, context=context)

    def create_future(self) -> asyncio.Future[Any]:
        return asyncio.Future[None](loop=self)

    def get_debug(self) -> bool:
        return False

    async def shutdown_asyncgens(self) -> None:
        pass

    async def shutdown_default_executor(self, timeout: float | None = None) -> None:
        pass


class WasiEventLoopPolicy(asyncio.AbstractEventLoopPolicy):
    def __init__(self) -> None:
        self._loop: asyncio.AbstractEventLoop | None = None

    def get_event_loop(self) -> asyncio.AbstractEventLoop:
        if self._loop is None:
            self._loop = self.new_event_loop()
        return self._loop

    def set_event_loop(self, loop: asyncio.AbstractEventLoop | None) -> None:
        self._loop = loop

    def new_event_loop(self) -> asyncio.AbstractEventLoop:
        return PollLoop()  # type: ignore[abstract]


def _iter[T](runner: asyncio.Runner, it: "AsyncGenerator[T]") -> "Generator[T]":
    loop = runner.get_loop()
    if not isinstance(loop, PollLoop):
        raise RuntimeError("runner.get_loop() must return a PollLoop")
    try:
        yield from loop.run_async_generator(it)
    finally:
        runner.close()


@overload
def run[T](main: "Coroutine[Any, Any, T]") -> T: ...
@overload
def run[T](main: "AsyncGenerator[T, None]") -> "Generator[T]": ...
def run[T](main: "Coroutine[Any, Any, T] | AsyncGenerator[T]") -> "T | Generator[T]":
    runner = asyncio.Runner()
    if hasattr(main, "__aiter__"):
        return _iter(runner, cast("AsyncGenerator[T]", main))
    with runner:
        return runner.run(cast("Coroutine[None, None, T]", main))


async def _aiter_arg(args: "_promptkit_sys.ArgIter") -> "AsyncGenerator[Any]":
    while True:
        ok, result, poll = args.read()
        if not ok:
            break
        elif poll is not None:
            await subscribe(poll)
        else:
            yield result
