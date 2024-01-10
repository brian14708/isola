import { useQuery, type FetchQueryOptions } from '@tanstack/react-query';

export const getUserProfileQuery: FetchQueryOptions<{
	id: string;
	name: string;
	profile: {
		avatar_url: string | null;
	};
}> = {
	queryKey: ['/user/me'],
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
};

export const useUserProfile = () => {
	return useQuery(getUserProfileQuery);
};
