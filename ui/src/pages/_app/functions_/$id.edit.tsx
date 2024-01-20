import Editor from '@/components/Editor';
import { useTheme } from '@/hooks/theme';
import { FileRoute } from '@tanstack/react-router';

export const Route = new FileRoute('/_app/functions/$id/edit').createRoute({
	component: Page
});

function Page() {
	const { theme } = useTheme();
	return (
		<Editor
			wrapperProps={{
				className: 'flex-1'
			}}
			theme={theme.current === 'dark' ? 'vs-dark' : 'vs-light'}
			defaultLanguage="python"
			defaultValue="# some comment"
		/>
	);
}
