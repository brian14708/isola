import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as fsp from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import type { RuntimeName } from "./types.js";

const ASSET_FILES: Record<RuntimeName, string> = {
  python: "python.wasm.zst",
  js: "js.wasm.zst",
};

const RELEASE_API =
  "https://api.github.com/repos/brian14708/isola/releases/tags/{version}";

export interface RuntimeConfig {
  runtimePath: string;
}

function cacheBase(): string {
  return process.env.XDG_CACHE_HOME ?? path.join(os.homedir(), ".cache");
}

function pkgVersion(): string {
  // Search upward from this ESM module for the package's own package.json.
  // During tests the module may be rooted at src/, so also check npm/.
  let dir = path.dirname(fileURLToPath(import.meta.url));
  for (let i = 0; i < 4; i++) {
    for (const candidate of [
      path.join(dir, "package.json"),
      path.join(dir, "npm", "package.json"),
    ]) {
      try {
        const pkg = JSON.parse(fs.readFileSync(candidate, "utf-8")) as {
          name?: string;
          version: string;
        };
        if (pkg.name === "isola-core") return pkg.version;
      } catch {}
    }
    dir = path.dirname(dir);
  }
  throw new Error("could not determine isola-core version from package.json");
}

function versionTag(ver: string): string {
  if (ver.startsWith("v")) return ver;
  return `v${ver}`;
}

export async function resolveRuntime(
  runtime: RuntimeName,
  version?: string,
): Promise<RuntimeConfig> {
  const ver = version ?? pkgVersion();
  const cacheDir = path.join(
    cacheBase(),
    "isola",
    "runtimes",
    `${runtime}-${ver}`,
  );
  const assetName = ASSET_FILES[runtime];
  const assetPath = path.join(cacheDir, assetName);

  if (await isCached(assetPath)) {
    return { runtimePath: cacheDir };
  }

  const expectedDigest = await fetchExpectedDigest(ver, assetName);
  const wasmBytes = await downloadAsset(ver, assetName, expectedDigest);
  await fsp.mkdir(cacheDir, { recursive: true });
  const staging = await fsp.mkdtemp(path.join(cacheDir, ".runtime-"));
  try {
    const stagedAsset = path.join(staging, assetName);
    await fsp.writeFile(stagedAsset, wasmBytes);
    await fsp.writeFile(`${stagedAsset}.sha256`, expectedDigest);
    await fsp.rename(stagedAsset, assetPath);
    await fsp.rename(`${stagedAsset}.sha256`, `${assetPath}.sha256`);
  } finally {
    await fsp.rm(staging, { recursive: true, force: true });
  }
  return { runtimePath: cacheDir };
}

async function isCached(assetPath: string): Promise<boolean> {
  try {
    const expected = await fsp.readFile(`${assetPath}.sha256`, "utf8");
    const hash = crypto.createHash("sha256");
    for await (const chunk of fs.createReadStream(assetPath)) {
      hash.update(chunk);
    }
    return expected === `sha256:${hash.digest("hex")}`;
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT" || code === "EISDIR") {
      return false;
    }
    throw error;
  }
}

async function fetchExpectedDigest(
  version: string,
  assetName: string,
): Promise<string> {
  const url = RELEASE_API.replace("{version}", versionTag(version));
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(
      `failed to fetch release info: ${resp.status} ${resp.statusText}`,
    );
  }
  const release = (await resp.json()) as {
    assets: Array<{ name: string; digest?: string }>;
  };

  for (const asset of release.assets ?? []) {
    if (asset.name === assetName) {
      if (!asset.digest) {
        throw new Error(
          `no digest found for asset '${assetName}' in release ${version}`,
        );
      }
      return asset.digest;
    }
  }
  throw new Error(`asset '${assetName}' not found in release ${version}`);
}

async function downloadAsset(
  version: string,
  assetName: string,
  expectedDigest: string,
): Promise<Buffer> {
  const url = `https://github.com/brian14708/isola/releases/download/${versionTag(version)}/${assetName}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(
      `failed to download asset: ${resp.status} ${resp.statusText}`,
    );
  }

  const data = Buffer.from(await resp.arrayBuffer());
  const actual = `sha256:${crypto.createHash("sha256").update(data).digest("hex")}`;
  if (actual !== expectedDigest) {
    throw new Error(
      `digest mismatch for ${assetName}: expected ${expectedDigest}, got ${actual}`,
    );
  }

  return data;
}
