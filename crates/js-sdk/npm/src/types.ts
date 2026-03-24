export type JsonScalar = boolean | number | string | null;
export type JsonValue = JsonScalar | JsonValue[] | { [key: string]: JsonValue };
export type RuntimeName = "python" | "js";

export interface TemplateOptions {
  runtimePath?: string;
  version?: string;
  cacheDir?: string | null;
  maxMemory?: number | null;
  prelude?: string | null;
  runtimeLibDir?: string | null;
  mounts?: MountConfig[];
  env?: Record<string, string>;
}

export interface MountConfig {
  host: string;
  guest: string;
  dir_perms?: "read" | "write" | "read-write";
  file_perms?: "read" | "write" | "read-write";
}

export interface SandboxOptions {
  maxMemory?: number | null;
  mounts?: MountConfig[];
  env?: Record<string, string>;
  hostcalls?: Record<string, (payload: JsonValue) => Promise<unknown>>;
  http?: HttpHandlerConfig;
  httpHandler?: HttpHandlerConfig;
}

export interface HttpRequest {
  method: string;
  url: string;
  headers: Record<string, string>;
  body: Buffer | null;
}

export interface HttpResponse {
  status: number;
  headers?: Record<string, string>;
  body?: Buffer | null;
}

export type HttpHandler = (req: HttpRequest) => Promise<HttpResponse>;
export type HttpHandlerConfig = HttpHandler | true;

export interface ResultEvent {
  type: "result";
  data: JsonValue;
}

export interface EndEvent {
  type: "end";
  data: JsonValue | null;
}

export interface StdoutEvent {
  type: "stdout";
  data: string;
}

export interface StderrEvent {
  type: "stderr";
  data: string;
}

export interface ErrorEvent {
  type: "error";
  data: string;
}

export interface LogEvent {
  type: "log";
  data: string;
}

export type Event =
  | ResultEvent
  | EndEvent
  | StdoutEvent
  | StderrEvent
  | ErrorEvent
  | LogEvent;

export class Arg {
  constructor(
    public value: unknown,
    public name?: string,
  ) {}
}

export type RunArg = JsonValue | Arg;
export type RunKwargs = Record<string, RunArg>;
