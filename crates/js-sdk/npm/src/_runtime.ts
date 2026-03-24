import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as fsp from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as zlib from "node:zlib";

import type { RuntimeName } from "./types.js";

const BUNDLE_FILES: Record<RuntimeName, string> = {
  python: "python.wasm",
  js: "js.wasm",
};

const TARBALL_NAMES: Record<RuntimeName, string> = {
  python: "isola-python-runtime.tar.gz",
  js: "isola-js-runtime.tar.gz",
};

const RELEASE_API =
  "https://api.github.com/repos/brian14708/isola/releases/tags/{version}";

export interface RuntimeConfig {
  runtimePath: string;
  runtimeLibDir?: string;
}

function cacheBase(): string {
  return process.env.XDG_CACHE_HOME ?? path.join(os.homedir(), ".cache");
}

function pkgVersion(): string {
  // Search upward from __dirname for the package's own package.json.
  // At runtime __dirname is npm/dist/; during tests (vitest/vite) it is the
  // source root, so we also check a nested npm/package.json as we walk up.
  let dir = __dirname;
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
  if (ver.startsWith("v") || ver === "latest") return ver;
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
  const checkPath = path.join(cacheDir, "bin", BUNDLE_FILES[runtime]);

  try {
    await fsp.access(checkPath, fs.constants.F_OK);
    return buildConfig(runtime, cacheDir);
  } catch {
    // not cached, fall through to download
  }

  const tarballName = TARBALL_NAMES[runtime];
  const expectedDigest = await fetchExpectedDigest(ver, tarballName);
  const tarballBytes = await downloadTarball(ver, tarballName, expectedDigest);
  await extractTarball(tarballBytes, cacheDir);

  try {
    await fsp.access(checkPath, fs.constants.F_OK);
  } catch {
    throw new Error(`downloaded runtime is missing '${BUNDLE_FILES[runtime]}'`);
  }

  return buildConfig(runtime, cacheDir);
}

function buildConfig(runtime: RuntimeName, cacheDir: string): RuntimeConfig {
  if (runtime === "python") {
    return {
      runtimePath: path.join(cacheDir, "bin"),
      runtimeLibDir: path.join(cacheDir, "lib"),
    };
  }
  return { runtimePath: path.join(cacheDir, "bin") };
}

async function fetchExpectedDigest(
  version: string,
  tarballName: string,
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
    if (asset.name === tarballName) {
      if (!asset.digest) {
        throw new Error(
          `no digest found for asset '${tarballName}' in release ${version}`,
        );
      }
      return asset.digest;
    }
  }
  throw new Error(`asset '${tarballName}' not found in release ${version}`);
}

async function downloadTarball(
  version: string,
  tarballName: string,
  expectedDigest: string,
): Promise<Buffer> {
  const url = `https://github.com/brian14708/isola/releases/download/${versionTag(version)}/${tarballName}`;
  const resp = await fetch(url);
  if (!resp.ok) {
    throw new Error(
      `failed to download tarball: ${resp.status} ${resp.statusText}`,
    );
  }

  const data = Buffer.from(await resp.arrayBuffer());
  const actual = `sha256:${crypto.createHash("sha256").update(data).digest("hex")}`;
  if (actual !== expectedDigest) {
    throw new Error(
      `digest mismatch for ${tarballName}: expected ${expectedDigest}, got ${actual}`,
    );
  }

  return data;
}

async function extractTarball(data: Buffer, dest: string): Promise<void> {
  const tarData = await new Promise<Buffer>((resolve, reject) => {
    zlib.gunzip(data, (err, result) => {
      if (err) reject(err);
      else resolve(result);
    });
  });

  await fsp.mkdir(path.dirname(dest), { recursive: true });
  const tmpDir = await fsp.mkdtemp(
    path.join(path.dirname(dest), `.${path.basename(dest)}-`),
  );
  try {
    await parseTar(tarData, tmpDir);
    try {
      await fsp.rename(tmpDir, dest);
    } catch (err: unknown) {
      const code = (err as NodeJS.ErrnoException).code;
      if (code === "EEXIST" || code === "ENOTEMPTY") {
        await fsp.rm(tmpDir, { recursive: true, force: true });
      } else {
        throw err;
      }
    }
  } catch (err) {
    await fsp.rm(tmpDir, { recursive: true, force: true });
    throw err;
  }
}

async function parseTar(data: Buffer, dest: string): Promise<void> {
  const BLOCK = 512;
  let offset = 0;

  while (offset + BLOCK <= data.length) {
    const hdr = data.subarray(offset, offset + BLOCK);
    offset += BLOCK;

    if (hdr.every((b) => b === 0)) break;

    const name = nullTerm(hdr, 0, 100);
    const modeStr = nullTerm(hdr, 100, 108).trim();
    const sizeStr = nullTerm(hdr, 124, 136).trim();
    const typeFlag = String.fromCharCode(hdr[156]);
    const linkName = nullTerm(hdr, 157, 257);
    const prefix = nullTerm(hdr, 345, 500);

    const fullName = prefix ? `${prefix}/${name}` : name;
    const size = sizeStr ? parseInt(sizeStr, 8) : 0;
    const fileData = data.subarray(offset, offset + size);
    offset += Math.ceil(size / BLOCK) * BLOCK;

    const stripped = stripFirstComponent(fullName);
    if (!stripped) continue;

    const outPath = path.join(dest, stripped);
    if (!outPath.startsWith(dest + path.sep)) {
      throw new Error(`path traversal in archive: ${fullName}`);
    }

    if (typeFlag === "5" || (typeFlag === "\0" && name.endsWith("/"))) {
      await fsp.mkdir(outPath, { recursive: true });
    } else if (typeFlag === "2") {
      await fsp.mkdir(path.dirname(outPath), { recursive: true });
      try {
        await fsp.unlink(outPath);
      } catch {}
      await fsp.symlink(linkName, outPath);
    } else if (typeFlag === "0" || typeFlag === "\0") {
      await fsp.mkdir(path.dirname(outPath), { recursive: true });
      await fsp.writeFile(outPath, fileData);
      if (modeStr) await fsp.chmod(outPath, parseInt(modeStr, 8) & 0o7777);
    }
  }
}

function nullTerm(buf: Buffer, start: number, end: number): string {
  return buf.toString("utf8", start, end).replace(/\0.*$/, "");
}

function stripFirstComponent(p: string): string | null {
  const parts = p
    .replace(/\\/g, "/")
    .replace(/^\//, "")
    .split("/")
    .filter((s) => s.length > 0);
  if (parts.length <= 1) return null;
  const rest = parts.slice(1);
  if (rest.some((s) => s === "." || s === "..")) {
    throw new Error(`invalid path in archive: ${p}`);
  }
  return rest.join(path.sep);
}
