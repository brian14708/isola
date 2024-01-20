import { queryOptions } from '@tanstack/react-query';

interface FuncType {
	id: string;
	endpoint: string | null;
	name: string;
	visibility: 'public' | 'private' | 'internal';
}

export const listFunctionsQueryOptions = (page: number | undefined, pageSize: number) =>
	queryOptions<{
		functions: FuncType[];
		total: number;
	}>({
		queryKey: ['functions', 'list', page ?? 1, pageSize],
		queryFn: () =>
			fetch(`/api/functions?offset=${((page ?? 1) - 1) * pageSize || 0}&count=${pageSize}`).then(
				(res) => res.json()
			),
		staleTime: 15 * 1000
	});

export const getFunctionQueryOptions = (id: string) =>
	queryOptions<{
		function: FuncType;
	}>({
		queryKey: ['functions', 'get', id],
		queryFn: async () => {
			const d = await fetch('/api/functions/' + encodeURIComponent(id), {
				method: 'GET',
				headers: {
					'Content-Type': 'application/json'
				}
			});
			return await d.json();
		}
	});

export const listRevisionsQueryOptions = (id: string, page: number | undefined, pageSize: number) =>
	queryOptions<object>({
		queryKey: ['functions', 'revisions', 'list', page ?? 1, pageSize],
		queryFn: () =>
			fetch(
				`/api/functions/${id}/revisions?offset=${((page ?? 1) - 1) * pageSize || 0}&count=${pageSize}`
			).then((res) => res.json()),
		staleTime: 15 * 1000
	});
