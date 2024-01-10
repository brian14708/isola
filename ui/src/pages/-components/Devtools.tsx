import React from 'react';

const production = process.env.NODE_ENV === 'production';

const TanStackRouterDevtools = production
	? () => null
	: React.lazy(() =>
			import('@tanstack/router-devtools').then((res) => ({
				default: res.TanStackRouterDevtools
			}))
		);

const ReactQueryDevtools = production
	? () => null
	: React.lazy(() =>
			import('@tanstack/react-query-devtools').then((res) => ({
				default: res.ReactQueryDevtools
			}))
		);

export default function Devtools() {
	if (production) {
		return null;
	}

	return (
		<React.Suspense fallback={null}>
			<TanStackRouterDevtools position="bottom-left" />
			<ReactQueryDevtools buttonPosition="bottom-right" />
		</React.Suspense>
	);
}
