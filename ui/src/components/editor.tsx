import React from "react";
import {
  loader,
  Editor as MonacoEditor,
  EditorProps as MonacoEditorProps,
} from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import editorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import jsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";

import { useTheme } from "./theme-provider";

export interface EditorProps extends MonacoEditorProps {
  onSave?: () => void;
  onExecute?: () => void;
}

self.MonacoEnvironment = {
  getWorker(_, label) {
    if (label === "json") {
      return new jsonWorker();
    }
    return new editorWorker();
  },
};

monaco.editor.defineTheme("pk-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [],
  colors: {
    "editor.background": "#0c0a09",
  },
});

monaco.languages.json.jsonDefaults.setDiagnosticsOptions({
  comments: "ignore",
  trailingCommas: "ignore",
});

loader.config({ monaco });
loader.init();

export const Editor = React.forwardRef<
  monaco.editor.IStandaloneCodeEditor,
  EditorProps
>(function Editor(props, ref) {
  const { resolvedTheme } = useTheme();

  return (
    <MonacoEditor
      {...props}
      theme={props.theme || (resolvedTheme === "dark" ? "vs-dark" : "vs-light")}
      onMount={(editor, monaco) => {
        if (ref) {
          if (typeof ref === "function") {
            ref(editor);
          } else {
            ref.current = editor;
          }
        }

        const { onSave, onExecute, onMount } = props;
        if (onSave) {
          editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => {
            onSave();
          });
        }
        if (onExecute) {
          editor.addCommand(
            monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter,
            () => {
              onExecute();
            },
          );
        }
        onMount?.(editor, monaco);
      }}
    />
  );
});
