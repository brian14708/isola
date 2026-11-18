from __future__ import annotations

import asyncio
import hashlib
import os
import tempfile
from importlib.metadata import version as _pkg_version
from pathlib import Path
from typing import Literal

import httpx2

from isola._core import TemplateConfig

RuntimeName = Literal["python", "js"]

_ASSET_FILES: dict[str, str] = {"python": "python.wasm.zst", "js": "js.wasm.zst"}
_RELEASE_API = "https://api.github.com/repos/brian14708/isola/releases/tags/{version}"


def _version_tag(ver: str) -> str:
    if ver.startswith("v"):
        return ver
    return f"v{ver}"


def _cache_base() -> Path:
    xdg = os.environ.get("XDG_CACHE_HOME")
    if xdg:
        return Path(xdg)
    return Path.home() / ".cache"


async def resolve_runtime(
    runtime: RuntimeName, *, version: str | None = None
) -> TemplateConfig:
    if runtime not in _ASSET_FILES:
        msg = f"unknown runtime: {runtime!r}"
        raise ValueError(msg)

    if version is None:
        version = _pkg_version("isola")

    cache_dir = _cache_base() / "isola" / "runtimes" / f"{runtime}-{version}"
    asset_name = _ASSET_FILES[runtime]
    asset_path = cache_dir / asset_name

    if not await asyncio.to_thread(_is_cached, asset_path):
        expected_digest = await _fetch_expected_digest(version, asset_name)
        wasm_bytes = await _download_asset(version, asset_name, expected_digest)
        await asyncio.to_thread(_publish_asset, asset_path, wasm_bytes, expected_digest)

    return TemplateConfig(runtime_path=cache_dir)


def _is_cached(asset: Path) -> bool:
    try:
        expected = asset.with_suffix(".zst.sha256").read_bytes()
        with asset.open("rb") as file:
            digest = hashlib.file_digest(file, "sha256").hexdigest()
    except (FileNotFoundError, IsADirectoryError):
        return False
    return expected == f"sha256:{digest}".encode("ascii")


def _publish_asset(asset: Path, data: bytes, digest: str) -> None:
    asset.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".runtime-", dir=asset.parent) as tmp:
        staged = Path(tmp) / asset.name
        staged_digest = staged.with_suffix(".zst.sha256")
        staged.write_bytes(data)
        staged_digest.write_bytes(digest.encode("ascii"))
        staged.replace(asset)
        staged_digest.replace(asset.with_suffix(".zst.sha256"))


async def _fetch_expected_digest(version: str, asset_name: str) -> str:
    url = _RELEASE_API.format(version=_version_tag(version))
    async with httpx2.AsyncClient() as client:
        resp = await client.get(url)
        resp.raise_for_status()
        release = resp.json()

    for asset in release.get("assets", []):
        if asset.get("name") == asset_name:
            digest: str | None = asset.get("digest")
            if digest is None:
                msg = f"no digest found for asset {asset_name!r} in release {version}"
                raise RuntimeError(msg)
            return digest

    msg = f"asset {asset_name!r} not found in release {version}"
    raise RuntimeError(msg)


async def _download_asset(version: str, asset_name: str, expected_digest: str) -> bytes:
    download_url = (
        "https://github.com/brian14708/isola"
        f"/releases/download/{_version_tag(version)}/{asset_name}"
    )
    sha = hashlib.sha256()
    chunks: list[bytes] = []

    async with (
        httpx2.AsyncClient(follow_redirects=True) as client,
        client.stream("GET", download_url) as resp,
    ):
        resp.raise_for_status()
        async for chunk in resp.aiter_bytes():
            sha.update(chunk)
            chunks.append(chunk)

    actual = f"sha256:{sha.hexdigest()}"
    if actual != expected_digest:
        msg = (
            f"digest mismatch for {asset_name}: "
            f"expected {expected_digest}, got {actual}"
        )
        raise RuntimeError(msg)

    return b"".join(chunks)
