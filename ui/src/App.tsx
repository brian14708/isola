import Editor from '@monaco-editor/react';
import './monaco.config';

const template = `def handle(request):
	return { "message": "Hello world" }
`;

function App() {
	return <Editor height="90vh" defaultLanguage="python" defaultValue={template} />;
}

export default App;
