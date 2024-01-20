import { queryOptions } from '@tanstack/react-query';

export const getUserProfileQueryOptions = queryOptions<{
	id: string;
	name: string;
	profile: {
		avatar_url: string | null;
	};
}>({
	queryKey: ['users', 'me'],
	queryFn: async () => {
		const d = await fetch('/api/user/me', {
			method: 'GET',
			headers: {
				'Content-Type': 'application/json'
			},
			credentials: 'include'
		});
		if (d.status !== 200) {
			return null;
		}
		return await d.json();
	},
	staleTime: 15 * 1000
});
