"""Fixture for the --async200-stdlib preset."""
import asyncio
import os
import subprocess
import time
import urllib.request
from pathlib import Path


async def bad(path: Path):
    time.sleep(1)
    open("x")
    data = path.read_text()
    subprocess.run(["ls"])
    os.path.exists("/tmp")
    urllib.request.urlopen("http://example.com")
    os.system("ls")

    await anyio_path.read_text()  # awaited async wrapper: exempt
    await asyncio.to_thread(time.sleep, 1)  # passed, not called: exempt
    time.sleep(2)  # noqa: ASYNC200


async def custom_wins():
    time.sleep(3)  # user pattern overrides preset suggestion


def sync_ok(path: Path):
    time.sleep(1)  # sync context: not flagged lexically
    return path.read_text()
