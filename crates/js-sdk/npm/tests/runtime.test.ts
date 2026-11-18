import * as crypto from "node:crypto";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";

import { afterEach, describe, expect, it, vi } from "vitest";
import { resolveRuntime } from "../src/_runtime.js";

describe("resolveRuntime", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    delete process.env.XDG_CACHE_HOME;
  });

  it("repairs a partial runtime and reuses a verified cache entry", async () => {
    const cacheRoot = await fs.mkdtemp(path.join(os.tmpdir(), "isola-cache-"));
    process.env.XDG_CACHE_HOME = cacheRoot;
    const wasm = Buffer.from("wasm");
    const digest = `sha256:${crypto.createHash("sha256").update(wasm).digest("hex")}`;

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL | Request) => {
        const url = String(input);
        if (url.includes("/repos/brian14708/isola/releases/tags/v9.9.9")) {
          return new Response(
            JSON.stringify({ assets: [{ name: "python.wasm.zst", digest }] }),
            { status: 200 },
          );
        }
        if (url.includes("/releases/download/v9.9.9/python.wasm.zst")) {
          return new Response(wasm, { status: 200 });
        }
        throw new Error(`unexpected fetch url: ${url}`);
      }),
    );

    try {
      const cache = path.join(cacheRoot, "isola", "runtimes", "python-9.9.9");
      const asset = path.join(cache, "python.wasm.zst");
      await fs.mkdir(cache, { recursive: true });
      await fs.writeFile(asset, "partial");
      const config = await resolveRuntime("python", "9.9.9");
      expect(config.runtimePath).toBe(cache);
      expect(await fs.readFile(asset)).toEqual(wasm);
      await resolveRuntime("python", "9.9.9");
      expect(fetch).toHaveBeenCalledTimes(2);
      await fs.writeFile(asset, "truncated");
      await resolveRuntime("python", "9.9.9");
      expect(await fs.readFile(asset)).toEqual(wasm);
      expect(fetch).toHaveBeenCalledTimes(4);
    } finally {
      await fs.rm(cacheRoot, { recursive: true, force: true });
    }
  });
});
