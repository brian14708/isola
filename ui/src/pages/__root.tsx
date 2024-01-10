import { Outlet, rootRouteWithContext, useNavigate } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { NextUIProvider } from '@nextui-org/react';
import Devtools from './-components/Devtools';
import Navbar from './-components/Navbar';

export const Route = rootRouteWithContext<{
	queryClient: QueryClient;
}>()({
	component: RootComponent
});

function RootComponent() {
	const navigate = useNavigate();
	return (
		<NextUIProvider
			navigate={(path: string) =>
				navigate({
					to: path
				})
			}
		>
			<Navbar />
			<div className="flex">
				<Outlet />
			</div>
			<Devtools />
		</NextUIProvider>
	);
}
