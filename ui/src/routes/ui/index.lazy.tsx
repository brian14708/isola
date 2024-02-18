import { useCallback, useEffect, useRef } from "react";
import { createLazyFileRoute } from "@tanstack/react-router";
import JSON5 from "json5";
import type { editor } from "monaco-editor";
import pako from "pako";

import { Editor } from "@/components/editor";
import { EditorMenu } from "@/components/editor-menu";
import { useToast } from "@/components/ui/use-toast";

export const Route = createLazyFileRoute("/ui/")({
  component: Index,
});

function encode(uint8array: Uint8Array) {
  const output = [];
  for (let i = 0, length = uint8array.length; i < length; i++) {
    output.push(String.fromCharCode(uint8array[i]));
  }
  const base64 = btoa(output.join(""));
  return base64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

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
  const { toast } = useToast();

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
    if (window.location.hash) {
      window.location.hash = "";
    }
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

      if (res.headers.get("content-type")?.includes("application/json")) {
        const data = await res.json();
        previewRef.current.setValue(JSON.stringify(data, null, 2));
      } else {
        const body = res.body?.getReader();
        if (!body) throw new Error("No body");

        let result = "";
        for (;;) {
          const { done, value } = await body.read();
          if (done) break;
          result += new TextDecoder().decode(value);
          previewRef.current.setValue(result);
        }
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
      const hash = window.location.hash.substring(1);
      if (hash) {
        // from url safe base64
        const data = pako.inflateRaw(
          Uint8Array.from(
            atob(hash.replace(/_/g, "/").replace(/-/g, "+")),
            (c) => c.charCodeAt(0),
          ),
        );
        const { script, request } = JSON.parse(new TextDecoder().decode(data));
        editorRef.current.setValue(script);
        requestRef.current.setValue(request);
        return;
      }
    } catch (err) {
      // ignore
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
          onEvent={async (evt) => {
            switch (evt.type) {
              case "run":
                execute();
                break;
              case "save":
                save();
                break;
              case "load":
                if (!evt.url) {
                  editorRef.current?.setValue(CODE_TEMPLATE);
                  requestRef.current?.setValue(REQUEST_TEMPLATE);
                  save();
                }
                break;
              case "share": {
                if (!editorRef.current || !requestRef.current) {
                  return;
                }

                const data = JSON.stringify({
                  script: editorRef.current.getValue(),
                  request: requestRef.current.getValue(),
                });
                const compressed = encode(pako.deflateRaw(data, { level: 9 }));
                const u = new URL(window.location.toString());
                u.hash = compressed;
                await navigator.clipboard.writeText(u.toString());
                console.log(u.toString());
                toast({
                  title: "Copied to clipboard",
                });
                break;
              }
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
              unicodeHighlight: { ambiguousCharacters: false },
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
              unicodeHighlight: { ambiguousCharacters: false },
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
              unicodeHighlight: { ambiguousCharacters: false },
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
