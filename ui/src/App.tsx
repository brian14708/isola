import Editor from '@monaco-editor/react';
import './monaco.config';

const template = `def handle(request):
	return { "message": "Hello world" }
`;

function App() {
	return (
		<div className="flex">
			<div className="flex-1">
				<Editor theme="vs-dark" height="100vh" defaultLanguage="python" defaultValue={template} />
			</div>
			<div className="flex flex-col flex-1">
				<Editor theme="vs-dark" height="60vh" defaultLanguage="javascript" defaultValue={`{}`} />
				<textarea className="flex-1" />
			</div>
		</div>
	);
}

export default App;
