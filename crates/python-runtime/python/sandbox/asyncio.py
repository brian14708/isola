from __future__ import annotations

import asyncio
import contextlib
import logging
import sys
import weakref
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

logger = logging.getLogger(__name__)

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
    __slots__: tuple[str, ...] = (
        "_asyncgens",
        "_finalizing_asyncgens",
        "closed",
        "handles",
        "running",
        "wakers",
    )

    def __init__(self) -> None:
        self.wakers: list[
            tuple[
                _isola_sys.Pollable[None],
                _isola_sys.Pollable[object],
                asyncio.Future[object] | asyncio.Handle,
            ]
        ] = []
        self.running: bool = False
        self.closed: bool = False
        self.handles: deque[asyncio.Handle] = deque()
        self._asyncgens: weakref.WeakSet[AsyncGenerator[object]] = weakref.WeakSet()
        self._finalizing_asyncgens: set[AsyncGenerator[object]] = set()

    def subscribe[T](self, pollable: _isola_sys.Pollable[T]) -> asyncio.Future[T]:
        waker = self.create_future()
        subscription = pollable.subscribe()
        if subscription is None:
            self._resume_future(pollable, waker)
        else:
            self.wakers.append((subscription, pollable, waker))
        return cast("asyncio.Future[T]", waker)

    @override
    def run_until_complete[T](self, future: Awaitable[T]) -> T:
        old_asyncgen_hooks = sys.get_asyncgen_hooks()
        try:
            self.running = True
            asyncio.events._set_running_loop(self)  # noqa: SLF001
            sys.set_asyncgen_hooks(
                firstiter=self._asyncgen_firstiter_hook,
                finalizer=self._asyncgen_finalizer_hook,
            )
            return self._run_until_complete(future)
        finally:
            self._cleanup()
            self._retain_asyncgens_for_shutdown()
            self.running = False
            sys.set_asyncgen_hooks(
                firstiter=old_asyncgen_hooks.firstiter,
                finalizer=old_asyncgen_hooks.finalizer,
            )
            asyncio.events._set_running_loop(None)  # noqa: SLF001

    def run_async_generator[T](self, generator: AsyncGenerator[T]) -> Generator[T]:
        it = aiter(generator)
        old_asyncgen_hooks = sys.get_asyncgen_hooks()
        try:
            self.running = True
            asyncio.events._set_running_loop(self)  # noqa: SLF001
            sys.set_asyncgen_hooks(
                firstiter=self._asyncgen_firstiter_hook,
                finalizer=self._asyncgen_finalizer_hook,
            )

            while True:
                try:
                    yield self._run_until_complete(anext(it), suspend=True)
                except StopAsyncIteration:
                    break
        finally:
            self._cleanup()
            self._retain_asyncgens_for_shutdown()
            self.running = False
            sys.set_asyncgen_hooks(
                firstiter=old_asyncgen_hooks.firstiter,
                finalizer=old_asyncgen_hooks.finalizer,
            )
            asyncio.events._set_running_loop(None)  # noqa: SLF001

    def _asyncgen_firstiter_hook(self, generator: AsyncGenerator[object]) -> None:
        self._asyncgens.add(generator)

    def _asyncgen_finalizer_hook(self, generator: AsyncGenerator[object]) -> None:
        self._asyncgens.discard(generator)
        if not self.closed:
            self._finalizing_asyncgens.add(generator)
            self.call_soon(self._finalize_asyncgen, generator)

    def _finalize_asyncgen(self, generator: AsyncGenerator[object]) -> None:
        self._finalizing_asyncgens.discard(generator)
        _ = self.create_task(generator.aclose())

    def _retain_asyncgens_for_shutdown(self) -> None:
        # Keep active generators alive after their top-level loop turn so
        # Runner.close() can deterministically pass them to shutdown_asyncgens().
        self._finalizing_asyncgens.update(self._asyncgens)
        self._asyncgens.clear()

    def _run_until_complete[T](
        self, future: Awaitable[T], *, suspend: bool = False
    ) -> T:
        task = cast("asyncio.Future[T]", asyncio.ensure_future(future, loop=self))

        def step() -> bool:
            while True:
                servicing_ready = False
                waiting_before_callbacks = False
                if self.wakers:
                    readyset = _isola_sys.ready(self.wakers)
                    servicing_ready = any(readyset)
                    self._dispatch_ready(readyset)
                    waiting_before_callbacks = bool(self.wakers)
                if task.done() or not self.running:
                    return False
                if not self.handles:
                    break
                for _ in range(len(self.handles)):
                    handle = self.handles.popleft()
                    if not handle._cancelled:  # noqa: SLF001
                        handle._run()  # noqa: SLF001
                if waiting_before_callbacks and not servicing_ready:
                    break

            if task.done() or not self.running:
                return False
            return bool(self.wakers)

        _isola_sys.drive(step, suspend)

        if not task.done() and self.running:
            msg = "Deadlock detected"
            raise RuntimeError(msg)
        return task.result()

    def _dispatch_ready(self, readyset: bytes) -> None:
        new_wakers: list[
            tuple[
                _isola_sys.Pollable[None],
                _isola_sys.Pollable[object],
                asyncio.Future[object] | asyncio.Handle,
            ]
        ] = []
        for is_ready, (subscription, pollable, waker) in zip(
            readyset, self.wakers, strict=True
        ):
            if not is_ready:
                new_wakers.append((subscription, pollable, waker))
            elif isinstance(waker, asyncio.Handle):
                pollable.release()
                subscription.release()
                self.handles.append(waker)
            else:
                self._resume_future(pollable, waker)
                subscription.release()
        self.wakers = new_wakers

    @staticmethod
    def _resume_future(
        pollable: _isola_sys.Pollable[object],
        waker: asyncio.Future[object],
    ) -> None:
        # Resolve the awaiting coroutine. `pollable.get()` raises when the
        # underlying host call failed; route that to the future so user code can
        # catch it at the `await` point instead of crashing the event loop.
        if not waker.cancelled():
            try:
                waker.set_result(pollable.get())
            except Exception as exc:  # noqa: BLE001
                waker.set_exception(exc)
        # Release regardless of cancellation so the host-side call slot is
        # always reclaimed.
        pollable.release()

    def _cleanup(self) -> None:
        while self.handles:
            self.handles.popleft().cancel()
        for subscription, pollable, waker in self.wakers:
            waker.cancel()
            pollable.release()
            subscription.release()
        self.wakers.clear()
        # Cancelling futures may enqueue their done callbacks.
        while self.handles:
            handle = self.handles.popleft()
            if not handle._cancelled:  # noqa: SLF001
                handle._run()  # noqa: SLF001

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
        message = cast(
            "str",
            context.get("message", "Unhandled exception in sandbox PollLoop"),
        )
        exception = context.get("exception")
        if isinstance(exception, BaseException):
            logger.error(
                message,
                exc_info=(type(exception), exception, exception.__traceback__),
            )
        else:
            logger.error(message)

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
        subscription = fut.subscribe()
        if subscription is None:
            fut.release()
            self.handles.append(handle)
        else:
            self.wakers.append((subscription, fut, handle))
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
            subscription, pollable, waker = w[i]
            if waker is handle:
                pollable.release()
                subscription.release()
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
        eager_start: bool | None = None,
    ) -> asyncio.Task[T]:
        if eager_start is None:
            return asyncio.Task(coro, loop=self, name=name, context=context)
        return asyncio.Task(
            coro,
            loop=self,
            name=name,
            context=context,
            eager_start=eager_start,
        )

    @override
    def create_future(self) -> asyncio.Future[object]:
        return asyncio.Future(loop=self)

    @override
    def get_debug(self) -> bool:
        return False

    @override
    async def shutdown_asyncgens(self) -> None:
        generators = [*self._asyncgens, *self._finalizing_asyncgens]
        self._asyncgens.clear()
        self._finalizing_asyncgens.clear()
        if not generators:
            return

        results = await asyncio.gather(
            *(generator.aclose() for generator in generators),
            return_exceptions=True,
        )
        for generator, result in zip(generators, results, strict=True):
            if isinstance(result, BaseException):
                self.call_exception_handler({
                    "message": "error while closing async generator",
                    "exception": result,
                    "asyncgen": generator,
                })

    @override
    async def shutdown_default_executor(self, timeout: float | None = None) -> None:
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
