import { beforeAll, describe, expect, it, vi } from "vitest";
import {
  Arg,
  buildTemplate,
  Sandbox,
  type SandboxTemplate,
} from "../src/index.js";
import type { Event } from "../src/types.js";

const RUNTIME_PATH = process.env.ISOLA_RUNTIME_PATH;
const RUNTIME_NAME = (process.env.ISOLA_RUNTIME_NAME ?? "python") as
  | "python"
  | "js";

const describeIfRuntime = RUNTIME_PATH ? describe : describe.skip;

describeIfRuntime("isola js-sdk", () => {
  let template: SandboxTemplate;

  beforeAll(async () => {
    template = await buildTemplate(RUNTIME_NAME, {
      // biome-ignore lint/style/noNonNullAssertion: guarded by describeIfRuntime
      runtimePath: RUNTIME_PATH!,
    });
  });

  it("should create a sandbox and run a function", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript("def add(a, b): return a + b");
    const result = await sandbox.run("add", [1, 2]);
    expect(result).toBe(3);
    sandbox.close();
  });

  it("should run a function returning a string", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript("def hello(name): return f'hello {name}'");
    const result = await sandbox.run("hello", ["world"]);
    expect(result).toBe("hello world");
    sandbox.close();
  });

  it("should return null for void function", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript("def noop(): pass");
    const result = await sandbox.run("noop");
    expect(result).toBeNull();
    sandbox.close();
  });

  it("should handle complex return types", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript(
      "def info(): return {'name': 'test', 'values': [1, 2, 3]}",
    );
    const result = await sandbox.run("info");
    expect(result).toEqual({ name: "test", values: [1, 2, 3] });
    sandbox.close();
  });

  it("should stream events", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript(
      ["def gen():", "    yield 1", "    yield 2"].join("\n"),
    );

    const events: Event[] = [];
    for await (const event of sandbox.runStream("gen")) {
      events.push(event);
    }

    const results = events.filter((e) => e.type === "result");
    const end = events.find((e) => e.type === "end");
    expect(results).toHaveLength(2);
    expect(results[0].data).toBe(1);
    expect(results[1].data).toBe(2);
    expect(end).toBeDefined();
    expect(end?.data).toBeNull();
    sandbox.close();
  });

  it("should support hostcalls", async () => {
    const sandbox = await template.create({
      hostcalls: {
        lookup: async (payload) => {
          const p = payload as { id: number };
          return { found: true, name: `user-${p.id}` };
        },
      },
    });
    await sandbox.start();
    await sandbox.loadScript(
      [
        "from sandbox.asyncio import hostcall",
        "",
        "async def lookup(user_id):",
        "    return await hostcall('lookup', {'id': user_id})",
      ].join("\n"),
    );
    const result = await sandbox.run("lookup", [42]);
    expect(result).toEqual({ found: true, name: "user-42" });
    sandbox.close();
  });

  it("should capture stdout", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript(
      [
        "def hello():",
        "    print('hello from sandbox')",
        "    return 'ok'",
      ].join("\n"),
    );

    const events: Event[] = [];
    for await (const event of sandbox.runStream("hello")) {
      events.push(event);
    }

    const stdoutEvents = events.filter((e) => e.type === "stdout");
    expect(stdoutEvents.length).toBeGreaterThan(0);
    sandbox.close();
  });

  it("should capture stderr", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript(
      [
        "import sys",
        "def warn():",
        "    print('error output', file=sys.stderr)",
        "    return 'done'",
      ].join("\n"),
    );

    const events: Event[] = [];
    for await (const event of sandbox.runStream("warn")) {
      events.push(event);
    }

    const stderrEvents = events.filter((e) => e.type === "stderr");
    expect(stderrEvents.length).toBeGreaterThan(0);
    sandbox.close();
  });

  it("should emit error event on exception", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript("def fail(): raise RuntimeError('oops')");

    const events: Event[] = [];
    for await (const event of sandbox.runStream("fail")) {
      events.push(event);
    }

    const errorEvents = events.filter((e) => e.type === "error");
    expect(errorEvents.length).toBeGreaterThan(0);
    sandbox.close();
  });

  it("should support named arguments via Arg", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript(
      "def greet(name, greeting='Hello'): return f'{greeting}, {name}!'",
    );
    const result = await sandbox.run("greet", [
      new Arg("World", "name"),
      new Arg("Hi", "greeting"),
    ]);
    expect(result).toBe("Hi, World!");
    sandbox.close();
  });

  it("should support Symbol.asyncDispose", async () => {
    const sandbox = await template.create();
    await sandbox.start();
    await sandbox.loadScript("def ping(): return 'ok'");
    const result = await sandbox.run("ping");
    expect(result).toBe("ok");
    await sandbox[Symbol.asyncDispose]();
  });

  it("should support http handler", async () => {
    const sandbox = await template.create({
      httpHandler: async (req) => {
        expect(req.method).toBe("GET");
        expect(req.url).toBe("https://example.test/hello");
        return {
          status: 200,
          headers: { "x-test": "node" },
          body: Buffer.from("response body"),
        };
      },
    });
    await sandbox.start();
    await sandbox.loadScript(
      [
        "from sandbox.http import fetch",
        "",
        "def main(url):",
        "    with fetch('GET', url) as resp:",
        "        data = b''.join(resp.iter_bytes())",
        "        return [resp.status, resp.headers.get('x-test'), data.decode()]",
      ].join("\n"),
    );
    const result = await sandbox.run("main", ["https://example.test/hello"]);
    expect(result).toEqual([200, "node", "response body"]);
    sandbox.close();
  });

  it("should preserve binary http response bodies", async () => {
    const responseBody = Buffer.from([0x00, 0xff, 0xc3, 0x28, 0x80, 0x41]);
    const sandbox = await template.create({
      httpHandler: async () => ({
        status: 200,
        body: responseBody,
      }),
    });
    await sandbox.start();
    await sandbox.loadScript(
      [
        "from sandbox.http import fetch",
        "",
        "def main(url):",
        "    with fetch('GET', url) as resp:",
        "        return list(b''.join(resp.iter_bytes()))",
      ].join("\n"),
    );
    const result = await sandbox.run("main", ["https://example.test/binary"]);
    expect(result).toEqual([...responseBody]);
    sandbox.close();
  });
});

describe("Arg", () => {
  it("should store value without name", () => {
    const a = new Arg(42);
    expect(a.value).toBe(42);
    expect(a.name).toBeUndefined();
  });

  it("should store value with name", () => {
    const a = new Arg("hello", "msg");
    expect(a.value).toBe("hello");
    expect(a.name).toBe("msg");
  });

  it("should be recognised by encodeArgs as a named arg", async () => {
    // Verify that Arg instances are encoded as named args by checking the
    // wire path: plain values get name=null, Arg instances get the name.
    // We can't run without a runtime, but we can verify the Arg shape.
    const a = new Arg({ x: 1 }, "opts");
    expect(a.value).toEqual({ x: 1 });
    expect(a.name).toBe("opts");
  });
});

describe("kwargs", () => {
  it("should merge positional args and kwargs in run", async () => {
    const run = vi.fn(async () => ({
      resultJson: [],
      finalJson: JSON.stringify(3),
      stdout: [],
      stderr: [],
      logs: [],
      errors: [],
    }));
    const sandbox = new Sandbox({
      run,
      close: vi.fn(),
      setCallback: vi.fn(),
    } as never);

    const result = await sandbox.run("add", [1], { b: 2 });

    expect(run).toHaveBeenCalledWith("add", [
      ["json", null, 1],
      ["json", "b", 2],
    ]);
    expect(result).toBe(3);
  });

  it("should support kwargs in runStream", async () => {
    let callback: ((kind: string, data: string | null) => void) | null = null;
    const run = vi.fn(async () => {
      callback?.("result", JSON.stringify("streamed"));
      return {
        resultJson: [],
        finalJson: JSON.stringify("done"),
        stdout: [],
        stderr: [],
        logs: [],
        errors: [],
      };
    });
    const sandbox = new Sandbox({
      run,
      close: vi.fn(),
      setCallback: vi.fn((next) => {
        callback = next as typeof callback;
      }),
    } as never);

    const events = [];
    for await (const event of sandbox.runStream("greet", [], {
      name: "World",
    })) {
      events.push(event);
    }

    expect(run).toHaveBeenCalledWith("greet", [["json", "name", "World"]]);
    expect(events).toEqual([
      { type: "result", data: "streamed" },
      { type: "end", data: "done" },
    ]);
  });

  it("should reject conflicting Arg names in kwargs", async () => {
    const run = vi.fn();
    const sandbox = new Sandbox({
      run,
      close: vi.fn(),
      setCallback: vi.fn(),
    } as never);

    await expect(
      sandbox.run("greet", [], {
        greeting: new Arg("Hi", "salutation"),
      }),
    ).rejects.toThrow(
      "keyword argument 'greeting' conflicts with explicit argument name 'salutation'",
    );
    expect(run).not.toHaveBeenCalled();
  });

  it("should reject kwargs passed as the second argument", async () => {
    const run = vi.fn();
    const sandbox = new Sandbox({
      run,
      close: vi.fn(),
      setCallback: vi.fn(),
    } as never);

    await expect(
      sandbox.run("greet", { name: "World" } as never),
    ).rejects.toThrow(
      "sandbox args must be an array; pass kwargs as the third argument",
    );
    expect(run).not.toHaveBeenCalled();
  });
});
