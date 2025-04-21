// W3C Trace Context implementation
// Format: {version}-{trace-id}-{parent-id}-{trace-flags}
// Example: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01

export interface TraceContext {
  traceId: string;
  spanId: string;
  traceparent: string;
}

function generateRandomHex(length: number): string {
  const bytes = new Uint8Array(length / 2);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export function generateTraceContext(): TraceContext {
  const traceId = generateRandomHex(32); // 128-bit trace ID
  const spanId = generateRandomHex(16); // 64-bit span ID
  const version = "00"; // Version 0
  const flags = "01"; // Sampled flag

  const traceparent = `${version}-${traceId}-${spanId}-${flags}`;

  return {
    traceId,
    spanId,
    traceparent,
  };
}

export function extractTraceId(traceparent: string): string | null {
  const parts = traceparent.split("-");
  if (parts.length !== 4) return null;
  return parts[1]; // trace-id is the second part
}
