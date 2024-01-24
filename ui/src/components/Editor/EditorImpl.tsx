import { EditorProps, loader, Editor as MonacoEditor } from '@monaco-editor/react';

import * as monaco from 'monaco-editor';
import editorWorker from 'monaco-editor/esm/vs/editor/editor.worker?worker';
import jsonWorker from 'monaco-editor/esm/vs/language/json/json.worker?worker';
import cssWorker from 'monaco-editor/esm/vs/language/css/css.worker?worker';
import htmlWorker from 'monaco-editor/esm/vs/language/html/html.worker?worker';
import tsWorker from 'monaco-editor/esm/vs/language/typescript/ts.worker?worker';
import React from 'react';

export interface EditorImplProps extends EditorProps {
	onSave?: () => void;
	onExecute?: () => void;
}

self.MonacoEnvironment = {
	getWorker(_, label) {
		if (label === 'json') {
			return new jsonWorker();
		}
		if (label === 'css' || label === 'scss' || label === 'less') {
			return new cssWorker();
		}
		if (label === 'html' || label === 'handlebars' || label === 'razor') {
			return new htmlWorker();
		}
		if (label === 'typescript' || label === 'javascript') {
			return new tsWorker();
		}
		return new editorWorker();
	}
};

loader.config({ monaco });
export const init = loader.init();

export default React.forwardRef<monaco.editor.IStandaloneCodeEditor, EditorImplProps>(
	function EditorImpl(props, ref) {
		return (
			<MonacoEditor
				{...props}
				onMount={(editor, monaco) => {
					if (ref) {
						if (typeof ref === 'function') {
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
						editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
							onExecute();
						});
					}
					onMount?.(editor, monaco);
				}}
			/>
		);
	}
);
