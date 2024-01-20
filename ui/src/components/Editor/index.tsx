import type { EditorProps } from '@monaco-editor/react';
import React from 'react';

const EditorImpl = React.lazy(() => import('./EditorImpl'));
export default function Editor(props: EditorProps) {
	return (
		<React.Suspense fallback={<div>Loading...</div>}>
			<EditorImpl {...props} />
		</React.Suspense>
	);
}
