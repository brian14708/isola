import { getUserProfileQuery } from '@/api/users';
import { FileRoute, Outlet, redirect } from '@tanstack/react-router';

export const Route = new FileRoute('/_app').createRoute({
	component: AppLayout,
	beforeLoad: async ({ context, location }) => {
		const d = await context.queryClient.fetchQuery(getUserProfileQuery);
		if (!d) {
			throw redirect({
				to: '/login',
				search: {
					redirect: location.href
				}
			});
		}
	}
});

function AppLayout() {
	return <Outlet />;
}
