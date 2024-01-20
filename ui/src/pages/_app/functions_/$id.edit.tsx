import { FileRoute } from '@tanstack/react-router';

export const Route = new FileRoute('/_app/functions/$id/edit').createRoute({
	component: Page
});

function Page() {
	return <div>Edit</div>;
}
