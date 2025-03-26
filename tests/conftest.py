import os
import pathlib

import pytest
import pytest_asyncio
from grpclib.client import Channel

from stub.promptkit.script.v1 import ScriptServiceStub


@pytest_asyncio.fixture
async def client():
    async with Channel("localhost", port=3000) as channel:
        yield ScriptServiceStub(channel)


@pytest.fixture
def datadir():
    datadir = os.path.join(os.path.dirname(__file__), "data")
    return pathlib.Path(datadir)
