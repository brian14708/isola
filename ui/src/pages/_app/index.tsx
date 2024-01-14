import { FileRoute, Navigate } from '@tanstack/react-router';

export const Route = new FileRoute('/_app/').createRoute({
	component: Page
});

function Page() {
	return <Navigate to="/functions/" />;
}
