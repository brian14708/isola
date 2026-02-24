from __future__ import annotations

import asyncio
import hashlib
import io
import os
import shutil
import tarfile
import tempfile
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Literal

import httpx

from isola._core import TemplateConfig

RuntimeName = Literal["python", "js"]

_BUNDLE_FILES: dict[str, str] = {"python": "python.wasm", "js": "js.wasm"}
_TARBALL_NAMES: dict[str, str] = {
    "python": "isola-python-runtime.tar.gz",
    "js": "isola-js-runtime.tar.gz",
}
_RELEASE_API = "https://api.github.com/repos/brian14708/isola/releases/tags/{version}"


def _cache_base() -> Path:
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        return Path(xdg)
    return Path.home() / ".cache"


async def resolve_runtime(
    runtime: RuntimeName, *, version: str | None = None
) -> TemplateConfig:
    if runtime not in _BUNDLE_FILES:
        msg = f"unknown runtime: {runtime!r}"
        raise ValueError(msg)

    if version is None:
        version = _pkg_version("isola")

    cache_dir = _cache_base() / "isola" / "runtimes" / f"{runtime}-{version}"
    bundle_file = _BUNDLE_FILES[runtime]
    check_path = cache_dir / "bin" / bundle_file

    if check_path.is_file():
        return _build_config(runtime, cache_dir)

    # Download and extract.
    tarball_name = _TARBALL_NAMES[runtime]
    expected_digest = await _fetch_expected_digest(version, tarball_name)
    tarball_bytes = await _download_tarball(version, tarball_name, expected_digest)
    await _extract_tarball(tarball_bytes, cache_dir)

    return _build_config(runtime, cache_dir)


def _build_config(runtime: RuntimeName, cache_dir: Path) -> TemplateConfig:
    if runtime == "python":
        return TemplateConfig(
            runtime_path=cache_dir / "bin", runtime_lib_dir=cache_dir / "lib"
        )
    return TemplateConfig(runtime_path=cache_dir / "bin")


async def _fetch_expected_digest(version: str, tarball_name: str) -> str:
    url = _RELEASE_API.format(version=version)
    async with httpx.AsyncClient() as client:
        resp = await client.get(url)
        resp.raise_for_status()
        release = resp.json()

    for asset in release.get("assets", []):
        if asset.get("name") == tarball_name:
            digest: str | None = asset.get("digest")
            if digest is None:
                msg = f"no digest found for asset {tarball_name!r} in release {version}"
                raise RuntimeError(msg)
            return digest

    msg = f"asset {tarball_name!r} not found in release {version}"
    raise RuntimeError(msg)


async def _download_tarball(
    version: str, tarball_name: str, expected_digest: str
) -> bytes:
    download_url = (
        "https://github.com/brian14708/isola"
        f"/releases/download/{version}/{tarball_name}"
    )
    sha = hashlib.sha256()
    chunks: list[bytes] = []

    async with (
        httpx.AsyncClient(follow_redirects=True) as client,
        client.stream("GET", download_url) as resp,
    ):
        resp.raise_for_status()
        async for chunk in resp.aiter_bytes():
            sha.update(chunk)
            chunks.append(chunk)

    actual = f"sha256:{sha.hexdigest()}"
    if actual != expected_digest:
        msg = (
            f"digest mismatch for {tarball_name}: "
            f"expected {expected_digest}, got {actual}"
        )
        raise RuntimeError(msg)

    return b"".join(chunks)


async def _extract_tarball(data: bytes, dest: Path) -> None:
    def _do_extract() -> None:
        dest.parent.mkdir(parents=True, exist_ok=True)
        tmp_dir = tempfile.mkdtemp(dir=dest.parent, prefix=f".{dest.name}-")
        try:
            with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as tf:
                tf.extractall(tmp_dir)  # noqa: S202
            Path(tmp_dir).rename(dest)
        except BaseException:
            shutil.rmtree(tmp_dir, ignore_errors=True)
            raise

    await asyncio.to_thread(_do_extract)
