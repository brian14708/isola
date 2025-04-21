import { createFileRoute } from "@tanstack/react-router";
import { CodeEditor } from "@/components/ui/code-editor";
import { Button } from "@/components/ui/button";
import { ScriptOutput } from "@/components/script-output";
import { useScriptExecution } from "@/hooks/use-script-execution";
import { usePersistedState } from "@/hooks/use-persisted-state";
import { useEffect } from "react";

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

  const handleRunScript = () => {
    runScript(script, timeout);
  };

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      // Check for both Enter and NumpadEnter
      const isEnter =
        event.key === "Enter" || event.code === "Enter" || event.code === "NumpadEnter";
      const isModified = event.ctrlKey || event.metaKey;

      console.log("Global key pressed:", {
        key: event.key,
        code: event.code,
        ctrlKey: event.ctrlKey,
        metaKey: event.metaKey,
        isEnter,
        isModified,
        target: event.target?.constructor?.name,
      });

      if (isModified && isEnter) {
        event.preventDefault();
        event.stopPropagation();
        console.log("Running script via global keyboard shortcut");
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
              options={{
                lineNumbers: "on",
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                fontSize: 14,
              }}
              onMount={(editor, monaco) => {
                // Add keyboard shortcut directly to Monaco editor
                editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
                  console.log("Monaco keyboard shortcut triggered");
                  runScript(script, timeout);
                });

                // Also try adding a direct key binding
                editor.onKeyDown((e) => {
                  if ((e.ctrlKey || e.metaKey) && e.code === "Enter") {
                    e.preventDefault();
                    e.stopPropagation();
                    console.log("Monaco onKeyDown triggered");
                    runScript(script, timeout);
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
            </div>
          </div>
        </div>

        <ScriptOutput output={output} isRunning={isRunning} />
      </div>
    </div>
  );
}
