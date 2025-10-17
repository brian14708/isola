import { Editor, type EditorProps } from "@monaco-editor/react";
import { cva, type VariantProps } from "class-variance-authority";
import { useRef, useEffect, useCallback, forwardRef, useImperativeHandle } from "react";
import type * as Monaco from "monaco-editor";

import { cn } from "@/lib/utils";
import { LspClient, type LspClientNotifications } from "@/lib/lsp-client";
import type { CompletionItem } from "vscode-languageserver-protocol";
import { useTheme } from "@/components/theme-provider";
import { useRpcClient } from "@/lib/rpc";
import {
  ContentType,
  ScriptInlineSchema,
  ScriptService,
  SourceSchema,
} from "@/lib/rpc/promptkit/script/v1/service_pb";
import { create, toJson } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";

export interface CodeEditorRef {
  focus: () => void;
  selectRange: (range: Monaco.IRange) => void;
  getEditor: () => Monaco.editor.IStandaloneCodeEditor | null;
}

const codeEditorVariants = cva("rounded-md border bg-background text-foreground overflow-hidden", {
  variants: {
    size: {
      sm: "h-32",
      md: "h-64",
      lg: "h-96",
      xl: "h-[32rem]",
      full: "h-full",
    },
  },
  defaultVariants: {
    size: "md",
  },
});

interface CodeEditorProps
  extends Omit<EditorProps, "className">,
    VariantProps<typeof codeEditorVariants> {
  className?: string;
  enableLsp?: boolean;
  onDiagnostics?: (diagnostics: Monaco.editor.IMarkerData[]) => void;
  onLspReady?: (isReady: boolean) => void;
}

const CodeEditor = forwardRef<CodeEditorRef, CodeEditorProps>(
  (
    {
      className,
      size,
      theme,
      language = "python",
      options = {},
      enableLsp = false,
      onDiagnostics,
      onLspReady,
      value,
      onChange,
      onMount,
      ...props
    },
    ref
  ) => {
    const { resolvedTheme } = useTheme();
    const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
    const lspClientRef = useRef<LspClient | null>(null);
    const monacoRef = useRef<typeof Monaco | null>(null);
    const disposablesRef = useRef<Monaco.IDisposable[]>([]);

    const monacoTheme = theme || (resolvedTheme === "dark" ? "vs-dark" : "vs");

    const defaultOptions = {
      minimap: { enabled: false },
      scrollBeyondLastLine: false,
      fontSize: 14,
      lineNumbers: "on" as const,
      roundedSelection: false,
      scrollbar: {
        vertical: "auto" as const,
        horizontal: "auto" as const,
      },
      automaticLayout: true,
      acceptSuggestionOnCommitCharacter: true,
      acceptSuggestionOnEnter: "on" as const,
      quickSuggestions: true,
      suggestOnTriggerCharacters: true,
      ...options,
    };

    useImperativeHandle(ref, () => ({
      focus: () => editorRef.current?.focus(),
      selectRange: (range: Monaco.IRange) => {
        if (editorRef.current) {
          editorRef.current.setSelection(range);
          editorRef.current.revealRangeInCenter(range);
        }
      },
      getEditor: () => editorRef.current,
    }));

    const convertMonacoPositionToLsp = useCallback((position: Monaco.Position) => {
      return {
        line: position.lineNumber - 1,
        character: position.column - 1,
      };
    }, []);

    const convertLspRangeToMonaco = useCallback(
      (range: {
        start: { line: number; character: number };
        end: { line: number; character: number };
      }): Monaco.Range => {
        return new monacoRef.current!.Range(
          range.start.line + 1,
          range.start.character + 1,
          range.end.line + 1,
          range.end.character + 1
        );
      },
      []
    );

    const setupLanguageProviders = useCallback(() => {
      if (
        !monacoRef.current ||
        !lspClientRef.current ||
        !editorRef.current ||
        language !== "python"
      ) {
        return;
      }

      const monaco = monacoRef.current;
      const lspClient = lspClientRef.current;
      const disposables = disposablesRef.current;

      // Hover provider
      disposables.push(
        monaco.languages.registerHoverProvider(language, {
          provideHover: async (model, position) => {
            const code = model.getValue();
            const lspPosition = convertMonacoPositionToLsp(position);
            const hoverInfo = await lspClient.getHoverInfo(code, lspPosition);

            if (hoverInfo?.contents) {
              const contents = Array.isArray(hoverInfo.contents)
                ? hoverInfo.contents
                : [hoverInfo.contents];

              return {
                contents: contents.map((content) => {
                  if (typeof content === "string") {
                    return { value: content };
                  }
                  return {
                    value: content.value || "",
                    isTrusted: true,
                    supportThemeIcons: true,
                  };
                }),
                range: hoverInfo.range ? convertLspRangeToMonaco(hoverInfo.range) : undefined,
              };
            }
            return null;
          },
        })
      );

      // Completion provider
      disposables.push(
        monaco.languages.registerCompletionItemProvider(language, {
          triggerCharacters: [".", " ", "(", ")", "[", "]", "{", "}", ":", ",", "="],
          provideCompletionItems: async (model, position) => {
            const code = model.getValue();
            const lspPosition = convertMonacoPositionToLsp(position);
            const completions = await lspClient.getCompletion(code, lspPosition);

            if (!completions) return { suggestions: [] };

            const items = Array.isArray(completions) ? completions : completions.items || [];

            return {
              suggestions: items.map((item) => ({
                label: item.label,
                kind: item.kind || monaco.languages.CompletionItemKind.Text,
                detail: item.detail,
                documentation: item.documentation,
                insertText: item.insertText || item.label,
                range:
                  item.textEdit && "range" in item.textEdit
                    ? convertLspRangeToMonaco(item.textEdit.range)
                    : {
                        insert: new monaco.Range(
                          position.lineNumber,
                          position.column,
                          position.lineNumber,
                          position.column
                        ),
                        replace: new monaco.Range(
                          position.lineNumber,
                          position.column,
                          position.lineNumber,
                          position.column
                        ),
                      },
              })),
            };
          },
          resolveCompletionItem: async (item) => {
            if (item.label && lspClient.resolveCompletion) {
              const resolved = await lspClient.resolveCompletion({
                label: item.label,
              } as CompletionItem);
              if (resolved) {
                return {
                  ...item,
                  detail: resolved.detail || item.detail,
                  documentation: resolved.documentation || item.documentation,
                };
              }
            }
            return item;
          },
        })
      );

      // Signature help provider
      disposables.push(
        monaco.languages.registerSignatureHelpProvider(language, {
          signatureHelpTriggerCharacters: ["(", ","],
          provideSignatureHelp: async (model, position) => {
            const code = model.getValue();
            const lspPosition = convertMonacoPositionToLsp(position);
            const signatureHelp = await lspClient.getSignatureHelp(code, lspPosition);

            if (!signatureHelp) return null;

            return {
              value: {
                signatures:
                  signatureHelp.signatures?.map((sig) => ({
                    label: sig.label,
                    documentation: sig.documentation,
                    parameters:
                      sig.parameters?.map((param) => ({
                        label: param.label,
                        documentation: param.documentation,
                      })) || [],
                  })) || [],
                activeSignature: signatureHelp.activeSignature || 0,
                activeParameter: signatureHelp.activeParameter || 0,
              },
              dispose: () => {},
            };
          },
        })
      );

      // Rename provider
      disposables.push(
        monaco.languages.registerRenameProvider(language, {
          provideRenameEdits: async (model, position, newName) => {
            const code = model.getValue();
            const lspPosition = convertMonacoPositionToLsp(position);
            const workspaceEdit = await lspClient.getRenameEdits(code, lspPosition, newName);

            if (!workspaceEdit?.changes) return null;

            const edits: Monaco.languages.IWorkspaceTextEdit[] = [];
            Object.entries(workspaceEdit.changes).forEach(([uri, textEdits]) => {
              textEdits.forEach((edit) => {
                edits.push({
                  resource: monaco.Uri.parse(uri),
                  textEdit: {
                    range: convertLspRangeToMonaco(edit.range),
                    text: edit.newText,
                  },
                  versionId: undefined,
                });
              });
            });

            return { edits };
          },
        })
      );
    }, [language, convertLspRangeToMonaco, convertMonacoPositionToLsp]);

    const client = useRpcClient(ScriptService);
    const initializeLsp = useCallback(async () => {
      if (!enableLsp || language !== "python") return;

      const files = client.executeServerStream({
        source: create(SourceSchema, {
          sourceType: {
            case: "scriptInline",
            value: create(ScriptInlineSchema, {
              script: `
import os
import zipfile

EXT = {'.py', '.pyi', '.typed'}

def main():
    r = '/usr/local/lib/python3.14/site-packages/'
    for root, _, files in os.walk(r):
        base = root[len(r):]
        for f in files:
            ext = os.path.splitext(f)[1]
            if ext not in EXT:
                continue
            with open(os.path.join(root, f)) as fobj:
                yield (os.path.join(base, f), fobj.read())

    with zipfile.ZipFile('/usr/local/lib/bundle-src.zip', 'r') as z:
        for entry in z.infolist():
            if not entry.is_dir():
                yield (entry.filename, z.read(entry).decode())
`,
              runtime: "python3",
            }),
          },
        }),
        spec: {
          timeout: { seconds: BigInt(10) },
          arguments: [],
          method: "main",
        },
        resultContentType: [ContentType.PROTOBUF_VALUE],
      });

      const fs = (async () => {
        const fs: Record<string, string> = {};
        for await (const response of files) {
          if (response.result?.resultType.case === "value") {
            const entries = toJson(ValueSchema, response.result?.resultType.value) as string[];
            for (let i = 0; i < entries.length; i += 2) {
              fs["/lib/" + entries[i]] = entries[i + 1];
            }
          }
        }
        return fs;
      })();

      try {
        const lspClient = new LspClient();
        lspClientRef.current = lspClient;

        const notifications: LspClientNotifications = {
          onWaitingForInitialization: (isWaiting) => {
            onLspReady?.(!isWaiting);
          },
          onDiagnostics: (diagnostics) => {
            if (onDiagnostics && monacoRef.current && editorRef.current) {
              const model = editorRef.current.getModel();
              if (model) {
                const markers: Monaco.editor.IMarkerData[] = diagnostics.map((diag) => ({
                  severity:
                    diag.severity === 1
                      ? monacoRef.current!.MarkerSeverity.Error
                      : diag.severity === 2
                        ? monacoRef.current!.MarkerSeverity.Warning
                        : diag.severity === 3
                          ? monacoRef.current!.MarkerSeverity.Info
                          : monacoRef.current!.MarkerSeverity.Hint,
                  message: diag.message,
                  startLineNumber: diag.range.start.line + 1,
                  startColumn: diag.range.start.character + 1,
                  endLineNumber: diag.range.end.line + 1,
                  endColumn: diag.range.end.character + 1,
                  source: diag.source,
                }));

                monacoRef.current.editor.setModelMarkers(model, "lsp", markers);
                onDiagnostics(markers);
              }
            }
          },
          onError: () => {
            onLspReady?.(false);
          },
        };

        lspClient.requestNotification(notifications);

        // Add timeout for initialization
        const initPromise = lspClient.initialize({
          fs: await fs,
          config: {
            extraPaths: ["/lib"],
          },
        });
        const timeoutPromise = new Promise((_, reject) =>
          setTimeout(() => reject(new Error("LSP initialization timeout after 30 seconds")), 30000)
        );

        await Promise.race([initPromise, timeoutPromise]);

        if (value) {
          await lspClient.updateTextDocument(value);
        }

        setupLanguageProviders();
      } catch {
        onLspReady?.(false);
      }
    }, [enableLsp, client, language, value, onDiagnostics, onLspReady, setupLanguageProviders]);

    const handleEditorMount = useCallback(
      async (editor: Monaco.editor.IStandaloneCodeEditor, monaco: typeof Monaco) => {
        editorRef.current = editor;
        monacoRef.current = monaco;

        if (enableLsp) {
          await initializeLsp();
        }
        onMount?.(editor, monaco);
      },
      [enableLsp, initializeLsp, onMount]
    );

    const handleEditorChange = useCallback(
      async (newValue: string | undefined) => {
        onChange?.(
          newValue,
          null as unknown as Parameters<NonNullable<EditorProps["onChange"]>>[1]
        );

        if (enableLsp && lspClientRef.current && newValue !== undefined) {
          try {
            await lspClientRef.current.updateTextDocument(newValue);
          } catch {
            // Ignore LSP update errors
          }
        }
      },
      [onChange, enableLsp]
    );

    useEffect(() => {
      return () => {
        // Cleanup disposables
        disposablesRef.current.forEach((disposable) => disposable.dispose());
        disposablesRef.current = [];

        // Cleanup LSP connection
        if (lspClientRef.current?.connection) {
          lspClientRef.current.connection.dispose();
        }
      };
    }, []);

    return (
      <div className={cn(codeEditorVariants({ size, className }))}>
        <Editor
          theme={monacoTheme}
          language={language}
          options={defaultOptions}
          value={value}
          onChange={handleEditorChange}
          onMount={handleEditorMount}
          {...props}
        />
      </div>
    );
  }
);

CodeEditor.displayName = "CodeEditor";

export { CodeEditor };
export type { CodeEditorProps };
