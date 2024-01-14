import { keepPreviousData, useQuery } from '@tanstack/react-query';

export const useListFunctions = (page: number) => {
	return useQuery<{
		functions: {
			id: string;
			endpoint: string | null;
			name: string;
			visibility: 'public' | 'private' | 'internal';
		}[];
	}>({
		queryKey: ['/functions', page],
		queryFn: () => fetch(`/api/functions?page=${page || 0}`).then((res) => res.json()),
		staleTime: 15 * 1000,
		placeholderData: keepPreviousData
	});
};
