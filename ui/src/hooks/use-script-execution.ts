import { useState, useCallback, useRef } from "react";
import { create } from "@bufbuild/protobuf";
import { useRpcClient } from "@/lib/rpc/index";
import {
  ScriptService,
  ExecuteServerStreamRequestSchema,
  type ExecuteServerStreamResponse,
  SourceSchema,
  ScriptInlineSchema,
  ExecutionSpecSchema,
  ContentType,
} from "@/lib/rpc/promptkit/script/v1/service_pb";
import { TraceLevel } from "@/lib/rpc/promptkit/script/v1/trace_pb";
import { generateTraceContext } from "@/lib/trace-context";

export function useScriptExecution() {
  const client = useRpcClient(ScriptService);
  const [output, setOutput] = useState<string[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const streamRef = useRef<AsyncIterable<ExecuteServerStreamResponse> | null>(null);
  const abortControllerRef = useRef<AbortController | null>(null);

  const runScript = useCallback(
    async (script: string, timeout: number) => {
      if (isRunning) return;

      setIsRunning(true);
      setOutput([]);

      // Create new AbortController for this execution
      abortControllerRef.current = new AbortController();

      try {
        // Create execution request
        const traceContext = generateTraceContext();
        const source = create(SourceSchema, {
          sourceType: {
            case: "scriptInline",
            value: create(ScriptInlineSchema, {
              script,
              runtime: "python3",
              prelude: "",
            }),
          },
        });

        const spec = create(ExecutionSpecSchema, {
          timeout: timeout ? { seconds: BigInt(timeout), nanos: 0 } : undefined,
          arguments: [],
          method: "main",
          traceLevel: TraceLevel.ALL,
        });

        const request = create(ExecuteServerStreamRequestSchema, {
          source,
          spec,
          resultContentType: [ContentType.JSON],
        });

        const stream = client.executeServerStream(request, {
          headers: {
            traceparent: traceContext.traceparent,
          },
          signal: abortControllerRef.current.signal,
        });
        streamRef.current = stream;

        setOutput([`🔍 Trace ID: ${traceContext.traceId}`, ""]);

        for await (const response of stream) {
          // Check if execution was interrupted
          if (abortControllerRef.current?.signal.aborted) {
            setOutput((prev) => [...prev, "⚠️ Execution interrupted by user"]);
            break;
          }

          // Handle trace logs
          if (response.metadata?.traces) {
            for (const trace of response.metadata.traces) {
              const timestamp = new Date(
                Number(trace.timestamp?.seconds || 0) * 1000
              ).toLocaleTimeString();

              if (trace.traceType.case === "log") {
                const log = trace.traceType.value;
                setOutput((prev) => [...prev, `[${timestamp}] ${log.content}`]);
              } else if (trace.traceType.case === "event") {
                const event = trace.traceType.value;
                setOutput((prev) => [...prev, `[${timestamp}] Event: ${event.kind}`]);
              } else if (trace.traceType.case === "spanBegin") {
                const span = trace.traceType.value;
                setOutput((prev) => [...prev, `[${timestamp}] → ${span.kind}`]);
              } else if (trace.traceType.case === "spanEnd") {
                setOutput((prev) => [...prev, `[${timestamp}] ←`]);
              }
            }
          }

          // Handle results
          if (response.result?.resultType.case === "json") {
            const result = JSON.parse(response.result.resultType.value);
            setOutput((prev) => [...prev, `Result: ${JSON.stringify(result, null, 2)}`]);
          } else if (response.result?.resultType.case === "error") {
            const error = response.result.resultType.value;
            setOutput((prev) => [...prev, `Error: ${error.message}`]);
          }
        }
      } catch (error) {
        if (abortControllerRef.current?.signal.aborted) {
          setOutput((prev) => [...prev, "⚠️ Execution interrupted by user"]);
        } else {
          setOutput((prev) => [...prev, `Connection error: ${error}`]);
        }
      } finally {
        setIsRunning(false);
        streamRef.current = null;
        abortControllerRef.current = null;
      }
    },
    [client, isRunning]
  );

  const interruptExecution = useCallback(() => {
    console.log("Interrupt called, isRunning:", isRunning);
    if (isRunning && abortControllerRef.current) {
      console.log("Aborting execution...");
      setOutput((prev) => [...prev, "🛑 Stopping execution..."]);
      abortControllerRef.current.abort();
    }
  }, [isRunning]);

  return {
    output,
    isRunning,
    runScript,
    interruptExecution,
  };
}
