import * as crypto from "node:crypto";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as zlib from "node:zlib";

import { afterEach, describe, expect, it, vi } from "vitest";
import { resolveRuntime } from "../src/_runtime.js";

describe("resolveRuntime", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    delete process.env.XDG_CACHE_HOME;
  });

  it("extracts GNU long-name tar entries", async () => {
    const cacheRoot = await fs.mkdtemp(path.join(os.tmpdir(), "isola-cache-"));
    process.env.XDG_CACHE_HOME = cacheRoot;

    const longPath =
      "isola-python-runtime/lib/python3.13/site-packages/" +
      "very_long_package_name_with_nested_components_to_force_gnu_longlink_runtime_fixture/" +
      "example.txt";
    const tarball = zlib.gzipSync(
      Buffer.concat([
        tarFile("isola-python-runtime/bin/python.wasm", Buffer.from("wasm")),
        tarLongFile(longPath, Buffer.from("long-link payload")),
        Buffer.alloc(1024, 0),
      ]),
    );
    const digest = `sha256:${crypto.createHash("sha256").update(tarball).digest("hex")}`;

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL | Request) => {
        const url = String(input);
        if (url.includes("/repos/brian14708/isola/releases/tags/v9.9.9")) {
          return new Response(
            JSON.stringify({
              assets: [{ name: "isola-python-runtime.tar.gz", digest }],
            }),
            { status: 200 },
          );
        }

        if (
          url.includes("/releases/download/v9.9.9/isola-python-runtime.tar.gz")
        ) {
          return new Response(tarball, { status: 200 });
        }

        throw new Error(`unexpected fetch url: ${url}`);
      }),
    );

    try {
      const config = await resolveRuntime("python", "9.9.9");

      expect(
        await fs.readFile(path.join(config.runtimePath, "python.wasm")),
      ).toEqual(Buffer.from("wasm"));
      expect(
        await fs.readFile(
          path.join(
            // biome-ignore lint/style/noNonNullAssertion: python config always has runtimeLibDir
            config.runtimeLibDir!,
            "python3.13/site-packages/" +
              "very_long_package_name_with_nested_components_to_force_gnu_longlink_runtime_fixture/" +
              "example.txt",
          ),
        ),
      ).toEqual(Buffer.from("long-link payload"));
    } finally {
      await fs.rm(cacheRoot, { recursive: true, force: true });
    }
  });
});

function tarLongFile(name: string, data: Buffer): Buffer {
  return Buffer.concat([
    tarEntry("././@LongLink", Buffer.from(`${name}\0`), "L"),
    tarEntry("placeholder", data, "0"),
  ]);
}

function tarFile(name: string, data: Buffer): Buffer {
  return tarEntry(name, data, "0");
}

function tarEntry(name: string, data: Buffer, typeFlag: string): Buffer {
  const bodySize = typeFlag === "5" ? 0 : data.length;
  const header = tarHeader(name, bodySize, typeFlag);
  const body = typeFlag === "5" ? Buffer.alloc(0) : data;
  const padding = Buffer.alloc((512 - (body.length % 512)) % 512, 0);
  return Buffer.concat([header, body, padding]);
}

function tarHeader(name: string, size: number, typeFlag: string): Buffer {
  const header = Buffer.alloc(512, 0);

  writeString(header, name, 0, 100);
  writeOctal(header, typeFlag === "5" ? 0o755 : 0o644, 100, 8);
  writeOctal(header, 0, 108, 8);
  writeOctal(header, 0, 116, 8);
  writeOctal(header, size, 124, 12);
  writeOctal(header, 0, 136, 12);
  header.fill(0x20, 148, 156);
  header[156] = typeFlag.charCodeAt(0);
  writeString(header, "ustar", 257, 6);
  writeString(header, "00", 263, 2);

  let checksum = 0;
  for (const byte of header) checksum += byte;
  header.write(`${checksum.toString(8).padStart(6, "0")}\0 `, 148, 8, "ascii");

  return header;
}

function writeOctal(
  buffer: Buffer,
  value: number,
  start: number,
  size: number,
): void {
  buffer.write(
    `${value.toString(8).padStart(size - 1, "0")}\0`,
    start,
    size,
    "ascii",
  );
}

function writeString(
  buffer: Buffer,
  value: string,
  start: number,
  size: number,
): void {
  const bytes = Buffer.from(value, "utf8");
  if (bytes.length > size) {
    throw new Error(`tar field is too small for '${value}'`);
  }
  bytes.copy(buffer, start);
}
