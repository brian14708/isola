import type { EditorProps } from '@monaco-editor/react';
import React from 'react';

const EditorImpl = React.lazy(async () => {
	const mod = await import('./EditorImpl');
	await mod.init;
	return mod;
});

export default function Editor(props: EditorProps) {
	return (
		<React.Suspense fallback={<div>Loading...</div>}>
			<EditorImpl {...props} />
		</React.Suspense>
	);
}
