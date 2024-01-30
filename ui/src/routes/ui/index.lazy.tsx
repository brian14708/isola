import type { editor } from "monaco-editor";
import { createLazyFileRoute } from "@tanstack/react-router";

import { Editor } from "@/components/editor";
import { EditorMenu } from "@/components/editor-menu";
import { useCallback, useEffect, useRef } from "react";
import JSON5 from "json5";

export const Route = createLazyFileRoute("/ui/")({
  component: Index,
});

const CODE_TEMPLATE = `\
def handle(request):
	return request['data']
`;

const REQUEST_TEMPLATE = `\
{
  "method": "handle",
  "args": [
    {
      "data": "Hello",
    }
  ]
}`;

function Index() {
  const editorRef = useRef<editor.IStandaloneCodeEditor>(null);
  const requestRef = useRef<editor.IStandaloneCodeEditor>(null);
  const previewRef = useRef<editor.IStandaloneCodeEditor>(null);

  const save = useCallback(() => {
    if (!editorRef.current || !requestRef.current) {
      return;
    }

    window.localStorage.setItem(
      "editor",
      JSON.stringify({
        script: editorRef.current.getValue(),
        request: requestRef.current.getValue(),
      }),
    );
  }, []);

  useEffect(() => {
    const interval = setInterval(save, 10000);
    return () => clearInterval(interval);
  }, [save]);

  const execute = useCallback(async () => {
    if (!editorRef.current || !previewRef.current || !requestRef.current) {
      return;
    }
    previewRef.current.setValue("/* Loading... */");

    try {
      const m = JSON5.parse(requestRef.current.getValue()) as object;
      const res = await fetch("/v1/code/exec", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          ...m,
          script: editorRef.current.getValue(),
        }),
      });
      const body = res.body?.getReader();
      if (!body) throw new Error("No body");

      let result = "";
      for (;;) {
        const { done, value } = await body.read();
        if (done) break;
        result += new TextDecoder().decode(value);
        previewRef.current.setValue(result);
      }
    } catch (err) {
      previewRef.current.setValue(`/* ERROR: ${err} */`);
    }
  }, []);

  async function init() {
    if (!editorRef.current || !requestRef.current) {
      return;
    }

    try {
      const data = window.localStorage.getItem("editor");
      if (data) {
        const { script, request } = JSON.parse(data);
        editorRef.current.setValue(script);
        requestRef.current.setValue(request);
      }
    } catch (err) {
      // ignore
    }
  }

  return (
    <div className="flex h-screen w-screen flex-col">
      <div className="p-2">
        <EditorMenu
          onLoad={({ url }) => {
            if (!url) {
              editorRef.current?.setValue(CODE_TEMPLATE);
              requestRef.current?.setValue(REQUEST_TEMPLATE);
              save();
            }
          }}
        />
      </div>
      <div className="m-2 grid flex-1 grid-cols-5 grid-rows-4 gap-5">
        <div className="col-span-3 row-span-3 overflow-clip rounded-md border">
          <Editor
            ref={editorRef}
            onExecute={execute}
            onSave={save}
            options={{
              minimap: { enabled: false },
            }}
            language="python"
            defaultValue={CODE_TEMPLATE}
            onMount={(editor) => {
              editor.focus();
              init();
            }}
          />
        </div>
        <div className="col-span-2 row-span-3 overflow-clip rounded-md border">
          <Editor
            ref={requestRef}
            onExecute={execute}
            onSave={save}
            options={{
              minimap: { enabled: false },
              lineNumbers: "off",
              scrollbar: { vertical: "hidden" },
            }}
            language="json"
            onMount={() => {
              init();
            }}
            defaultValue={REQUEST_TEMPLATE}
          />
        </div>
        <div className="col-span-5 row-span-4 overflow-clip rounded-md border">
          <Editor
            ref={previewRef}
            options={{
              minimap: { enabled: false },
              lineNumbers: "off",
              readOnly: true,
            }}
            language="json"
          />
        </div>
      </div>
    </div>
  );
}
