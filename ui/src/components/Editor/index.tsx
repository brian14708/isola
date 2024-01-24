import { editor } from 'monaco-editor';
import React from 'react';
import type { EditorImplProps } from './EditorImpl';

const EditorImpl = React.lazy(async () => {
	const mod = await import('./EditorImpl');
	await mod.init;
	return mod;
});

export default React.forwardRef<editor.IStandaloneCodeEditor, EditorImplProps>(
	function Editor(props, ref) {
		return (
			<React.Suspense fallback={<div>Loading...</div>}>
				<EditorImpl ref={ref} {...props} />
			</React.Suspense>
		);
	}
);
