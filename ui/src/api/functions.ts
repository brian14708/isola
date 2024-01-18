import { useQuery } from '@tanstack/react-query';

export const useFunctionList = (page: number, pageSize: number) => {
	return useQuery<{
		functions: {
			id: string;
			endpoint: string | null;
			name: string;
			visibility: 'public' | 'private' | 'internal';
		}[];
		total: number;
	}>({
		queryKey: ['/functions', page, pageSize],
		queryFn: () =>
			fetch(`/api/functions?offset=${(page - 1) * pageSize || 0}&count=${pageSize}`).then((res) =>
				res.json()
			),
		staleTime: 15 * 1000
	});
};
