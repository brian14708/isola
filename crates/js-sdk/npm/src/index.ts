import { resolveRuntime } from "./_runtime.js";
// @ts-expect-error - napi bindings are generated at build time
import { ContextCore, type SandboxCore } from "./isola.js";
import type {
  Event,
  HttpHandler,
  HttpHandlerConfig,
  HttpRequest,
  HttpResponse,
  JsonValue,
  RunArg,
  RunKwargs,
  RuntimeName,
  SandboxOptions,
  TemplateOptions,
} from "./types.js";
import { Arg } from "./types.js";

export type {
  Event,
  HttpHandler,
  HttpHandlerConfig,
  HttpRequest,
  HttpResponse,
  JsonValue,
  MountConfig,
  RunArg,
  RunKwargs,
  RuntimeName,
  SandboxOptions,
  TemplateOptions,
} from "./types.js";

export { Arg } from "./types.js";

// ---------------------------------------------------------------------------
// Wire format helpers
// ---------------------------------------------------------------------------

type WireArgument = [string, string | null, unknown];
type NativeCallbackArgs = [string, string | null];
type NativeHostcallArgs = [string, string];
type NativeHttpArgs = [string, string, string, Buffer | null];
type NativeHttpResponse = {
  status: number;
  headers?: Record<string, string>;
  body?: Buffer | null;
};
type NativeRunResult = {
  resultJson: string[];
  finalJson?: string;
  stdout: string[];
  stderr: string[];
  logs: string[];
  errors: string[];
};

function encodeArgs(args?: RunArg[]): WireArgument[] {
  if (!args) return [];
  return args.map((arg): WireArgument => {
    if (arg instanceof Arg) {
      const a = arg as { value: unknown; name?: string };
      return ["json", a.name ?? null, a.value];
    }
    return ["json", null, arg];
  });
}

function normalizeKeywordArg(name: string, value: RunArg): Arg {
  if (value instanceof Arg) {
    if (value.name !== undefined && value.name !== name) {
      throw new TypeError(
        `keyword argument '${name}' conflicts with explicit argument name '${value.name}'`,
      );
    }
    return new Arg(value.value, name);
  }
  return new Arg(value, name);
}

function mergeRunArgs(args?: RunArg[], kwargs?: RunKwargs): RunArg[] {
  if (args !== undefined && !Array.isArray(args)) {
    throw new TypeError(
      "sandbox args must be an array; pass kwargs as the third argument",
    );
  }

  if (!kwargs) return args ? [...args] : [];

  return (args ? [...args] : []).concat(
    Object.entries(kwargs).map(([name, value]) =>
      normalizeKeywordArg(name, value),
    ),
  );
}

function unpackTuple<T extends unknown[]>(raw: unknown[]): T {
  if (raw.length === 1 && Array.isArray(raw[0])) {
    return raw[0] as T;
  }
  return raw as T;
}

function parseEvent(kind: string, data: string | null): Event | null {
  switch (kind) {
    case "result":
      return data != null
        ? { type: "result", data: JSON.parse(data) as JsonValue }
        : null;
    case "end":
      return {
        type: "end",
        data: data != null ? (JSON.parse(data) as JsonValue) : null,
      };
    case "stdout":
      return data != null ? { type: "stdout", data } : null;
    case "stderr":
      return data != null ? { type: "stderr", data } : null;
    case "error":
      return data != null ? { type: "error", data } : null;
    case "log":
      return data != null ? { type: "log", data } : null;
    default:
      return null;
  }
}

function eventKey(event: Event): string {
  return `${event.type}:${JSON.stringify(event.data)}`;
}

function resultToEvents(result: NativeRunResult): Event[] {
  const events: Event[] = [];

  for (const item of result.resultJson) {
    events.push({
      type: "result",
      data: JSON.parse(item) as JsonValue,
    });
  }

  for (const message of result.stdout) {
    events.push({ type: "stdout", data: message });
  }

  for (const message of result.stderr) {
    events.push({ type: "stderr", data: message });
  }

  for (const message of result.logs) {
    events.push({ type: "log", data: message });
  }

  for (const message of result.errors) {
    events.push({ type: "error", data: message });
  }

  events.push({
    type: "end",
    data:
      result.finalJson !== undefined
        ? (JSON.parse(result.finalJson) as JsonValue)
        : null,
  });

  return events;
}

async function defaultHttpHandler(request: HttpRequest): Promise<HttpResponse> {
  if (typeof fetch !== "function") {
    throw new Error(
      "global fetch is not available; pass a custom http handler",
    );
  }

  const response = await fetch(request.url, {
    method: request.method,
    headers: request.headers,
    body: request.body ?? undefined,
  });

  const body = Buffer.from(await response.arrayBuffer());
  return {
    status: response.status,
    headers: Object.fromEntries(response.headers.entries()),
    body,
  };
}

function resolveHttpHandler(
  http: HttpHandlerConfig | undefined,
): HttpHandler | undefined {
  if (http === undefined) return undefined;
  return http === true ? defaultHttpHandler : http;
}

// ---------------------------------------------------------------------------
// Top-level template helper
// ---------------------------------------------------------------------------

export async function buildTemplate(
  runtime: RuntimeName,
  options?: TemplateOptions,
): Promise<SandboxTemplate> {
  const context = new SandboxContext();
  return await context.compileTemplate(runtime, options);
}

// ---------------------------------------------------------------------------
// SandboxContext
// ---------------------------------------------------------------------------

export class SandboxContext {
  private _core: InstanceType<typeof ContextCore>;

  constructor() {
    this._core = new ContextCore();
  }

  async compileTemplate(
    runtime: RuntimeName,
    options?: TemplateOptions,
  ): Promise<SandboxTemplate> {
    let runtimePath = options?.runtimePath;
    let runtimeLibDir = options?.runtimeLibDir;
    if (!runtimePath) {
      const resolved = await resolveRuntime(runtime, options?.version);
      runtimePath = resolved.runtimePath;
      runtimeLibDir ??= resolved.runtimeLibDir;
    }

    const patch: Record<string, unknown> = {};
    if (options?.cacheDir !== undefined) patch.cache_dir = options.cacheDir;
    if (options?.maxMemory !== undefined) patch.max_memory = options.maxMemory;
    if (options?.prelude !== undefined) patch.prelude = options.prelude;
    if (runtimeLibDir !== undefined) patch.runtime_lib_dir = runtimeLibDir;
    if (options?.mounts !== undefined) patch.mounts = options.mounts;
    if (options?.env !== undefined) patch.env = options.env;

    if (Object.keys(patch).length > 0) {
      this._core.configure(patch);
    }

    await this._core.initializeTemplate(runtimePath, runtime);
    return new SandboxTemplate(this._core);
  }

  close(): void {
    this._core.close();
  }
}

// ---------------------------------------------------------------------------
// SandboxTemplate
// ---------------------------------------------------------------------------

export class SandboxTemplate {
  /** @internal */
  constructor(private _core: InstanceType<typeof ContextCore>) {}

  async create(options?: SandboxOptions): Promise<Sandbox> {
    const core: InstanceType<typeof SandboxCore> =
      await this._core.instantiate();
    const sandbox = new Sandbox(core);

    const configPatch: Record<string, unknown> = {};
    if (options?.maxMemory !== undefined)
      configPatch.max_memory = options.maxMemory;
    if (options?.mounts !== undefined) configPatch.mounts = options.mounts;
    if (options?.env !== undefined) configPatch.env = options.env;
    if (Object.keys(configPatch).length > 0) {
      core.configure(configPatch);
    }

    if (options?.hostcalls) {
      sandbox._setHostcalls(options.hostcalls);
    }
    if (options?.http !== undefined && options?.httpHandler !== undefined) {
      throw new TypeError("pass only one of http or httpHandler");
    }
    const http = resolveHttpHandler(options?.http ?? options?.httpHandler);
    if (http) {
      sandbox._setHttpHandler(http);
    }

    return sandbox;
  }
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

export class Sandbox {
  private _core: InstanceType<typeof SandboxCore>;

  /** @internal */
  constructor(core: InstanceType<typeof SandboxCore>) {
    this._core = core;
  }

  /** @internal */
  _setHostcalls(
    hostcalls: Record<string, (payload: JsonValue) => Promise<unknown>>,
  ): void {
    this._core.setHostcallHandler(
      async (...raw: unknown[]): Promise<string> => {
        const [callType, payloadJson] = unpackTuple<NativeHostcallArgs>(raw);
        const handler = hostcalls[callType];
        if (!handler) throw new Error(`unsupported hostcall: ${callType}`);
        const payload = JSON.parse(payloadJson) as JsonValue;
        const result = await handler(payload);
        return JSON.stringify(result);
      },
    );
  }

  /** @internal */
  _setHttpHandler(handler: (req: HttpRequest) => Promise<HttpResponse>): void {
    this._core.setHttpHandler(
      async (...raw: unknown[]): Promise<NativeHttpResponse> => {
        const [method, url, headersJson, body] =
          unpackTuple<NativeHttpArgs>(raw);
        const headers = JSON.parse(headersJson) as Record<string, string>;
        const req: HttpRequest = { method, url, headers, body };
        const resp = await handler(req);
        return {
          status: resp.status,
          headers: resp.headers,
          body: resp.body ?? null,
        };
      },
    );
  }

  async start(): Promise<void> {
    await this._core.start();
  }

  async loadScript(code: string): Promise<void> {
    await this._core.loadScript(code);
  }

  async run(
    name: string,
    args?: RunArg[],
    kwargs?: RunKwargs,
  ): Promise<JsonValue | null> {
    const encoded = encodeArgs(mergeRunArgs(args, kwargs));
    const result = await this._core.run(name, encoded);
    return result.finalJson
      ? (JSON.parse(result.finalJson) as JsonValue)
      : null;
  }

  async *runStream(
    name: string,
    args?: RunArg[],
    kwargs?: RunKwargs,
  ): AsyncGenerator<Event, void, undefined> {
    const queue: Event[] = [];
    const emitted = new Map<string, number>();
    let resolve: (() => void) | null = null;
    let done = false;
    let sawErrorEvent = false;
    let runResult: NativeRunResult | null = null;
    let runError: unknown = null;

    const wake = (): void => {
      if (resolve) {
        resolve();
        resolve = null;
      }
    };

    const pushEvent = (event: Event): void => {
      queue.push(event);
      emitted.set(eventKey(event), (emitted.get(eventKey(event)) ?? 0) + 1);
      if (event.type === "error") {
        sawErrorEvent = true;
      }
      wake();
    };

    this._core.setCallback((...raw: unknown[]) => {
      const [kind, data] = unpackTuple<NativeCallbackArgs>(raw);
      const event = parseEvent(kind, data);
      if (event) {
        pushEvent(event);
      }
    });

    const encoded = encodeArgs(mergeRunArgs(args, kwargs));
    const runPromise = this._core
      .run(name, encoded)
      .then((result: NativeRunResult) => {
        runResult = result;
      })
      .catch((err: unknown) => {
        runError = err;
      })
      .finally(() => {
        setImmediate(() => {
          done = true;
          wake();
        });
      });

    try {
      while (true) {
        while (queue.length > 0) {
          // biome-ignore lint/style/noNonNullAssertion: length checked above
          yield queue.shift()!;
        }
        if (done) break;
        await new Promise<void>((r) => {
          resolve = r;
        });
      }
      await runPromise;
      if (runResult) {
        const remaining = new Map(emitted);
        for (const event of resultToEvents(runResult)) {
          const key = eventKey(event);
          const count = remaining.get(key) ?? 0;
          if (count > 0) {
            remaining.set(key, count - 1);
            continue;
          }
          yield event;
        }
      }
      if (runError && !sawErrorEvent) {
        throw runError;
      }
    } finally {
      this._core.setCallback(null);
    }
  }

  close(): void {
    this._core.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    this.close();
  }
}
