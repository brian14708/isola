import React from 'react';
import ReactDOM from 'react-dom/client';
import { RouterProvider, Router } from '@tanstack/react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';

import ThemeProvider from '@/components/ThemeProvider';
import { routeTree } from './routeTree.gen';
import './index.css';

const queryClient = new QueryClient();

const router = new Router({
	routeTree,
	context: {
		queryClient
	},
	defaultPreload: 'intent'
});

ReactDOM.createRoot(document.getElementById('root')!).render(
	<React.StrictMode>
		<ThemeProvider>
			<QueryClientProvider client={queryClient}>
				<RouterProvider router={router} />
			</QueryClientProvider>
		</ThemeProvider>
	</React.StrictMode>
);
