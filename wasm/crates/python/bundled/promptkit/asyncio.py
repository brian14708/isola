import asyncio


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

            if self.wakers:
                [pollables, wakers] = list(map(list, zip(*self.wakers)))

                new_wakers = []
                ready = [False] * len(pollables)
                for index in poll.poll(pollables):
                    ready[index] = True

                for (ready, pollable), waker in zip(zip(ready, pollables), wakers):
                    if ready:
                        pollable.__exit__()
                        waker.set_result(None)
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
        self.running = False

    def shutdown_asyncgens(self):
        pass

    def call_exception_handler(self, context):
        self.exception = context.get("exception", None)

    def call_soon(self, callback, *args, context=None):
        handle = asyncio.Handle(callback, args, self, context)
        self.handles.append(handle)
        return handle

    def create_task(self, coro, *, name=None, context=None):
        return asyncio.Task(coro, loop=self, name=name, context=context)

    def create_future(self):
        return asyncio.Future(loop=self)

    def get_debug(self):
        return False


async def register(loop: PollLoop, pollable):
    waker = loop.create_future()
    loop.wakers.append((pollable, waker))
    await waker

def new_event_loop():
    return PollLoop()

_global_loop = None

def run(main):
    global _global_loop
    if _global_loop is None:
        _global_loop = new_event_loop()
    asyncio.set_event_loop(_global_loop)
    return _global_loop.run_until_complete(main)