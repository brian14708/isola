import Editor from '@/components/Editor';
import type { editor } from 'monaco-editor';
import { useTheme } from '@/hooks/theme';
import { FileRoute } from '@tanstack/react-router';
import { useRef } from 'react';

export const Route = new FileRoute('/_app/functions/$id/edit').createRoute({
	component: Page
});

const CODE_TEMPLATE = `\
import typing

class Request(typing.TypedDict):
	data: str

def handle(request: Request, ctx) -> str:
	return request['data']
`;

function Page() {
	const editorRef = useRef<editor.IStandaloneCodeEditor>(null);
	const previewRef = useRef<editor.IStandaloneCodeEditor>(null);
	const requestRef = useRef<editor.IStandaloneCodeEditor>(null);

	async function execute() {
		if (!editorRef.current || !previewRef.current || !requestRef.current) return;
		previewRef.current.setValue('/* Loading... */');

		try {
			const schema = await fetch('/api/schema', {
				method: 'POST',
				headers: {
					'Content-Type': 'application/json'
				},
				body: JSON.stringify({
					script: editorRef.current.getValue(),
					method: 'handle'
				})
			}).then((res) => res.json());
			console.log(schema);

			const res = await fetch('/api/exec', {
				method: 'POST',
				headers: {
					'Content-Type': 'application/json'
				},
				body: JSON.stringify({
					script: editorRef.current.getValue(),
					method: 'handle',
					args: [JSON.parse(requestRef.current.getValue()), {}]
				})
			});
			const body = res.body?.getReader();
			if (!body) throw new Error('No body');

			let result = '';
			while (true) {
				const { done, value } = await body.read();
				if (done) break;
				result += new TextDecoder().decode(value);
				previewRef.current.setValue(result);
			}
		} catch (err) {
			previewRef.current.setValue(`/* ERROR: ${err} */`);
		}
	}

	const { theme } = useTheme();
	return (
		<div className="flex-1 grid grid-cols-2 grid-rows-2">
			<Editor
				ref={editorRef}
				wrapperProps={{
					className: 'row-span-2'
				}}
				onExecute={execute}
				options={{
					minimap: {
						enabled: false
					}
				}}
				theme={theme.current === 'dark' ? 'vs-dark' : 'vs-light'}
				defaultLanguage="python"
				defaultValue={CODE_TEMPLATE}
			/>
			<Editor
				ref={requestRef}
				onExecute={execute}
				options={{
					minimap: {
						enabled: false
					}
				}}
				theme={theme.current === 'dark' ? 'vs-dark' : 'vs-light'}
				defaultLanguage="json"
				defaultValue={'{"data":"Hello"}'}
			/>
			<Editor
				ref={previewRef}
				onExecute={execute}
				options={{
					minimap: {
						enabled: false
					},
					readOnly: true
				}}
				theme={theme.current === 'dark' ? 'vs-dark' : 'vs-light'}
				defaultLanguage="json"
				defaultValue={'//\n{"a":12}\n{"a":12}'}
			/>
		</div>
	);
}
