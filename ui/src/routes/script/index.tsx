import { createFileRoute } from "@tanstack/react-router";
import { CodeEditor } from "@/components/ui/code-editor";
import { Button } from "@/components/ui/button";
import { ScriptOutput } from "@/components/script-output";
import { useScriptExecution } from "@/hooks/use-script-execution";
import { usePersistedState } from "@/hooks/use-persisted-state";
import { useEffect, useState, useCallback } from "react";
import type * as Monaco from "monaco-editor";

export const Route = createFileRoute("/script/")({
  component: RouteComponent,
});

function RouteComponent() {
  const [script, setScript] = usePersistedState(
    "promptkit-script",
    `# Write your script here
def main():
    print('Hello, PromptKit!')

if __name__ == '__main__':
    main()`
  );
  const [timeout, setTimeout] = usePersistedState("promptkit-timeout", 30);
  const { output, isRunning, runScript, interruptExecution } = useScriptExecution();

  const [lspReady, setLspReady] = useState(false);
  const [lspEnabled, setLspEnabled] = usePersistedState("promptkit-lsp-enabled", true);
  const [diagnostics, setDiagnostics] = useState<Monaco.editor.IMarkerData[]>([]);

  const handleRunScript = useCallback(() => {
    runScript(script, timeout);
  }, [runScript, script, timeout]);

  const handleDiagnostics = useCallback((newDiagnostics: Monaco.editor.IMarkerData[]) => {
    setDiagnostics(newDiagnostics);
  }, []);

  const handleLspReady = useCallback((isReady: boolean) => {
    setLspReady(isReady);
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      // Check for both Enter and NumpadEnter
      const isEnter =
        event.key === "Enter" || event.code === "Enter" || event.code === "NumpadEnter";
      const isModified = event.ctrlKey || event.metaKey;

      if (isModified && isEnter) {
        event.preventDefault();
        event.stopPropagation();
        runScript(script, timeout);
      }
    };

    // Try both capture and bubble phases
    document.addEventListener("keydown", handleKeyDown, true);
    document.addEventListener("keydown", handleKeyDown, false);
    window.addEventListener("keydown", handleKeyDown, true);

    return () => {
      document.removeEventListener("keydown", handleKeyDown, true);
      document.removeEventListener("keydown", handleKeyDown, false);
      window.removeEventListener("keydown", handleKeyDown, true);
    };
  }, [runScript, script, timeout]);

  return (
    <div className="flex h-full w-full flex-col">
      {/* Header */}
      <div className="flex-shrink-0 border-b p-4">
        <h1 className="text-2xl font-bold">Script Editor</h1>
      </div>

      <div className="flex min-h-0 flex-1 flex-col">
        {/* Top area with code editor and options */}
        <div className="flex min-h-0 flex-1">
          {/* Main coding area */}
          <div className="flex flex-1 flex-col">
            <CodeEditor
              size="full"
              language="python"
              value={script}
              onChange={(value) => setScript(value || "")}
              enableLsp={lspEnabled}
              onDiagnostics={handleDiagnostics}
              onLspReady={handleLspReady}
              options={{
                lineNumbers: "on",
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                fontSize: 14,
                wordWrap: "on",
                automaticLayout: true,
              }}
              onMount={(editor, monaco) => {
                // Add keyboard shortcut directly to Monaco editor
                editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
                  handleRunScript();
                });

                // Also try adding a direct key binding
                editor.onKeyDown((e) => {
                  if ((e.ctrlKey || e.metaKey) && e.code === "Enter") {
                    e.preventDefault();
                    e.stopPropagation();
                    handleRunScript();
                  }
                });
              }}
            />
          </div>

          {/* Options pane */}
          <div className="bg-muted/30 flex w-80 flex-col border-l">
            <div className="border-b p-4">
              <h2 className="text-lg font-semibold">Options</h2>
            </div>
            <div className="flex-1 space-y-4 overflow-auto p-4">
              {/* LSP Status */}
              <div className="rounded-md border p-3">
                <div className="flex items-center justify-between">
                  <span className="text-sm font-medium">Language Server</span>
                  <div className="flex items-center gap-2">
                    <div
                      className={`h-2 w-2 rounded-full ${
                        !lspEnabled ? "bg-gray-400" : lspReady ? "bg-green-500" : "bg-yellow-500"
                      }`}
                    />
                    <button
                      onClick={() => setLspEnabled(!lspEnabled)}
                      className="text-muted-foreground hover:text-foreground text-xs"
                    >
                      {lspEnabled ? "Disable" : "Enable"}
                    </button>
                  </div>
                </div>
                <p className="text-muted-foreground mt-1 text-xs">
                  {!lspEnabled ? "Disabled" : lspReady ? "Ready" : "Initializing..."}
                </p>
              </div>

              {/* Diagnostics */}
              {diagnostics.length > 0 && (
                <div className="rounded-md border p-3">
                  <h3 className="mb-2 text-sm font-medium">Issues ({diagnostics.length})</h3>
                  <div className="max-h-32 space-y-1 overflow-auto">
                    {diagnostics.map((diag, index) => (
                      <div key={index} className="text-xs">
                        <div
                          className={`mr-2 inline-block h-2 w-2 rounded-full ${
                            diag.severity === 8
                              ? "bg-red-500"
                              : diag.severity === 4
                                ? "bg-yellow-500"
                                : diag.severity === 2
                                  ? "bg-blue-500"
                                  : "bg-gray-500"
                          }`}
                        />
                        <span className="text-muted-foreground">Line {diag.startLineNumber}:</span>
                        <span>{diag.message}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              <div>
                <label className="text-sm font-medium">Timeout (seconds)</label>
                <input
                  type="number"
                  value={timeout}
                  onChange={(e) => setTimeout(parseInt(e.target.value) || 30)}
                  className="mt-1 w-full rounded border p-2"
                />
              </div>
              <Button
                className="w-full"
                onClick={isRunning ? interruptExecution : handleRunScript}
                variant={isRunning ? "destructive" : "default"}
              >
                {isRunning ? "Stop Script" : "Run Script"}
              </Button>

              {/* Keyboard shortcut hint */}
              <div className="text-muted-foreground border-t pt-2 text-center text-xs">
                Press <kbd className="bg-muted rounded px-1 py-0.5 text-xs">Ctrl+Enter</kbd> to run
              </div>
            </div>
          </div>
        </div>

        <ScriptOutput output={output} isRunning={isRunning} />
      </div>
    </div>
  );
}
