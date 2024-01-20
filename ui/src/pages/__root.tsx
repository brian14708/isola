import { Outlet, rootRouteWithContext, useNavigate } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { NextUIProvider } from '@nextui-org/react';
import { ToastContainer } from 'react-toastify';
import 'react-toastify/dist/ReactToastify.css';
import Devtools from './-components/Devtools';
import Navbar from './-components/Navbar';
import { useTheme } from '@/hooks/theme';

export const Route = rootRouteWithContext<{
	queryClient: QueryClient;
}>()({
	component: RootComponent
});

function RootComponent() {
	const navigate = useNavigate();
	const { theme } = useTheme();
	return (
		<NextUIProvider
			navigate={(path: string) =>
				navigate({
					to: path
				})
			}
			className="min-h-screen flex flex-col"
		>
			<Navbar />
			<Outlet />
			<Devtools />
			<ToastContainer position="top-center" theme={theme.current} />
		</NextUIProvider>
	);
}
