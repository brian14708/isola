import { BrowserMessageReader, BrowserMessageWriter } from "vscode-jsonrpc/browser";
import {
  CompletionItem,
  CompletionList,
  CompletionParams,
  CompletionRequest,
  CompletionResolveRequest,
  ConfigurationParams,
  Diagnostic,
  DiagnosticTag,
  DidChangeConfigurationParams,
  DidChangeTextDocumentParams,
  DidOpenTextDocumentParams,
  Hover,
  HoverParams,
  HoverRequest,
  MessageConnection,
  NotificationType,
  RequestType,
  createMessageConnection,
  InitializeParams,
  InitializeRequest,
  LogMessageParams,
  Position,
  PublishDiagnosticsParams,
  RenameParams,
  RenameRequest,
  SignatureHelp,
  SignatureHelpParams,
  SignatureHelpRequest,
  WorkspaceEdit,
  SemanticTokensParams,
  SemanticTokensRequest,
  SemanticTokens,
  InlayHintParams,
  InlayHintRequest,
  InlayHint,
  Range,
} from "vscode-languageserver-protocol";
import "remote-web-worker";

const packageName = "browser-basedpyright";

interface SessionOptions {
  fs?: Record<string, string>;
  config?: { [name: string]: unknown };
}

export interface LspClientNotifications {
  onWaitingForInitialization?: (isWaiting: boolean) => void;
  onDiagnostics?: (diag: Diagnostic[]) => void;
  onError?: (message: string) => void; // TODO
}

const rootPath = "/src/";

const rootUri = `file://${rootPath}`;

const fileName = "Untitled.py";

const documentUri = rootUri + fileName;

export class LspClient {
  public connection: MessageConnection | undefined;
  private _documentVersion = 1;
  private _documentText = "";
  private _notifications: LspClientNotifications = {};

  requestNotification(notifications: LspClientNotifications) {
    this._notifications = notifications;
  }

  public updateCode = (code: string) => [(this._documentText = code)];

  public async initialize(sessionOptions?: SessionOptions) {
    this._notifications.onWaitingForInitialization?.(true);

    const fs = sessionOptions?.fs || {};

    try {
      const workerScript = `https://cdn.jsdelivr.net/npm/${packageName}/dist/pyright.worker.js`;
      const foreground = new Worker(workerScript, {
        name: "Pyright-foreground",
        type: "classic",
      });

      // Add error handling for worker
      foreground.onerror = (error) => {
        this._notifications.onError?.(`Worker failed to load: ${error.message}`);
        this._notifications.onWaitingForInitialization?.(false);
      };
      foreground.postMessage({
        type: "browser/boot",
        mode: "foreground",
      });
      const connection = createMessageConnection(
        new BrowserMessageReader(foreground),
        new BrowserMessageWriter(foreground)
      );
      const workers: Worker[] = [foreground];
      connection.onDispose(() => {
        workers.forEach((w) => w.terminate());
      });

      let backgroundWorkerCount = 0;
      foreground.addEventListener("message", (e: MessageEvent) => {
        if (e.data && e.data.type === "browser/newWorker") {
          // Create a new background worker.
          // The foreground worker has created a message channel and passed us
          // a port. We create the background worker and pass transfer the port
          // onward.
          const { initialData, port } = e.data;
          const background = new Worker(workerScript, {
            name: `Pyright-background-${++backgroundWorkerCount}`,
          });
          workers.push(background);
          background.postMessage(
            {
              type: "browser/boot",
              mode: "background",
              initialData,
              port,
            },
            [port]
          );
        }
      });

      this.connection = connection;

      // Add connection error handling
      this.connection.onError((error) => {
        this._notifications.onError?.(`Connection error: ${error}`);
      });

      this.connection.onClose(() => {
        this._notifications.onWaitingForInitialization?.(false);
      });

      this.connection.listen();
      fs[rootPath + fileName] = this._documentText;
      fs[rootPath + "pyrightconfig.json"] = JSON.stringify({
        typeshedPath: "/typeshed",
        pythonVersion: "3.13",
        pythonPlatform: "All",
        ...sessionOptions?.config,
      });
      // Initialize the server.
      const init: InitializeParams = {
        rootUri,
        rootPath,
        processId: 1,
        capabilities: {
          textDocument: {
            publishDiagnostics: {
              tagSupport: {
                valueSet: [DiagnosticTag.Unnecessary, DiagnosticTag.Deprecated],
              },
              versionSupport: true,
            },
            hover: {
              contentFormat: ["markdown", "plaintext"],
            },
            signatureHelp: {},
          },
        },
        initializationOptions: {
          files: fs,
        },
      };

      await this.connection.sendRequest(InitializeRequest.type, init);

      // Update the settings.
      await this.connection.sendNotification(
        new NotificationType<DidChangeConfigurationParams>("workspace/didChangeConfiguration"),
        {
          settings: {},
        }
      );

      // Simulate an "open file" event.
      await this.connection.sendNotification(
        new NotificationType<DidOpenTextDocumentParams>("textDocument/didOpen"),
        {
          textDocument: {
            uri: documentUri,
            languageId: "python",
            version: this._documentVersion,
            text: this._documentText,
          },
        }
      );

      // Receive diagnostics from the language server.
      this.connection.onNotification(
        new NotificationType<PublishDiagnosticsParams>("textDocument/publishDiagnostics"),
        (diagInfo) => {
          this._notifications.onDiagnostics?.(diagInfo.diagnostics);
        }
      );

      // Log messages received by the language server for debugging purposes.
      this.connection.onNotification(
        new NotificationType<LogMessageParams>("window/logMessage"),
        () => {
          // Log messages from language server (ignored)
        }
      );

      // Handle requests for configurations.
      this.connection.onRequest(
        new RequestType<ConfigurationParams, unknown, unknown>("workspace/configuration"),
        () => {
          return [];
        }
      );
      this._notifications.onWaitingForInitialization?.(false);
    } catch (error) {
      this._notifications.onError?.(`Initialization failed: ${error}`);
      this._notifications.onWaitingForInitialization?.(false);
      throw error;
    }
  }

  updateSettings = async (sessionOptions: SessionOptions) => {
    this.connection?.dispose();
    await this.initialize(sessionOptions);
  };

  async getHoverInfo(code: string, position: Position): Promise<Hover | null> {
    if (this._documentText !== code) {
      await this.updateTextDocument(code);
    }

    const params: HoverParams = {
      textDocument: {
        uri: documentUri,
      },
      position,
    };

    if (!this.connection) return null;

    const result = await this.connection.sendRequest(HoverRequest.type, params).catch(() => {
      // Don't return an error. Just return null (no info).
      return null;
    });

    return result;
  }

  async getRenameEdits(
    code: string,
    position: Position,
    newName: string
  ): Promise<WorkspaceEdit | null> {
    if (this._documentText !== code) {
      await this.updateTextDocument(code);
    }

    const params: RenameParams = {
      textDocument: {
        uri: documentUri,
      },
      position,
      newName,
    };

    if (!this.connection) return null;

    const result = await this.connection.sendRequest(RenameRequest.type, params).catch(() => {
      // Don't return an error. Just return null (no edits).
      return null;
    });

    return result;
  }

  async getSignatureHelp(code: string, position: Position): Promise<SignatureHelp | null> {
    if (this._documentText !== code) {
      await this.updateTextDocument(code);
    }

    const params: SignatureHelpParams = {
      textDocument: {
        uri: documentUri,
      },
      position,
    };

    if (!this.connection) return null;

    const result = await this.connection
      .sendRequest(SignatureHelpRequest.type, params)
      .catch(() => {
        // Don't return an error. Just return null (no info).
        return null;
      });

    return result;
  }

  async getCompletion(
    code: string,
    position: Position
  ): Promise<CompletionList | CompletionItem[] | null> {
    if (this._documentText !== code) {
      await this.updateTextDocument(code);
    }

    const params: CompletionParams = {
      textDocument: {
        uri: documentUri,
      },
      position,
    };

    if (!this.connection) return null;

    const result = await this.connection.sendRequest(CompletionRequest.type, params).catch(() => {
      // Don't return an error. Just return null (no info).
      return null;
    });

    return result;
  }

  async resolveCompletion(completionItem: CompletionItem): Promise<CompletionItem | null> {
    if (!this.connection) return null;

    const result = await this.connection
      .sendRequest(CompletionResolveRequest.type, completionItem)
      .catch(() => {
        // Don't return an error. Just return null (no info).
        return null;
      });

    return result;
  }

  async getSemanticTokens(): Promise<SemanticTokens | null> {
    const params: SemanticTokensParams = {
      textDocument: {
        uri: documentUri,
      },
    };
    if (!this.connection) return null;

    try {
      return await this.connection.sendRequest(SemanticTokensRequest.type, params);
    } catch {
      // Don't return an error. Just return null (no info).
      return null;
    }
  }
  async getInlayHints(range: Range): Promise<InlayHint[] | null> {
    const params: InlayHintParams = {
      textDocument: {
        uri: documentUri,
      },
      range,
    };
    if (!this.connection) return null;

    try {
      return await this.connection.sendRequest(InlayHintRequest.type, params);
    } catch {
      // Don't return an error. Just return null (no info).
      return null;
    }
  }

  // Sends a new version of the text document to the language server.
  // It bumps the document version and returns the new version number.
  async updateTextDocument(code: string): Promise<number> {
    const documentVersion = ++this._documentVersion;
    this._documentText = code;

    // Send the updated text to the language server.
    if (!this.connection) {
      throw new Error("LSP connection not established");
    }

    return this.connection
      .sendNotification(
        new NotificationType<DidChangeTextDocumentParams>("textDocument/didChange"),
        {
          textDocument: {
            uri: documentUri,
            version: documentVersion,
          },
          contentChanges: [
            {
              text: code,
            },
          ],
        }
      )
      .then(() => {
        return documentVersion;
      })
      .catch((err) => {
        throw err;
      });
  }
}
