interface ScriptOutputProps {
  output: string[];
  isRunning: boolean;
}

export function ScriptOutput({ output, isRunning }: ScriptOutputProps) {
  return (
    <div className="bg-muted/30 flex h-64 flex-col border-t">
      <div className="border-b p-4">
        <h2 className="text-lg font-semibold">Output</h2>
      </div>
      <div className="min-h-0 flex-1 p-4">
        <div className="h-full overflow-auto rounded bg-black p-3 font-mono text-sm text-green-400">
          {output.length === 0 && !isRunning && (
            <div className="text-gray-500">
              No output yet. Press Ctrl+Enter (or Cmd+Enter on Mac) or click "Run Script" to
              execute.
            </div>
          )}
          {isRunning && <div className="text-yellow-400">Running script...</div>}
          {output.map((line, index) => (
            <div
              key={index}
              className={
                line.startsWith("🔍 Trace ID:")
                  ? "font-semibold text-amber-400"
                  : line.startsWith("Error:")
                    ? "text-red-400"
                    : line.startsWith("Result:")
                      ? "text-cyan-400"
                      : line.includes("Event:")
                        ? "text-blue-400"
                        : line.includes("→") || line.includes("←")
                          ? "text-purple-400"
                          : "text-green-400"
              }
            >
              {line}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
