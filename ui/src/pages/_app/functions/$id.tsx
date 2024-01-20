import { getFunctionQueryOptions, listRevisionsQueryOptions } from '@/api/functions';
import { Button, Code, Divider, Link } from '@nextui-org/react';
import { useQuery, useSuspenseQuery } from '@tanstack/react-query';
import { FileRoute } from '@tanstack/react-router';
import { Card, CardHeader, CardBody } from '@nextui-org/react';
import * as v from 'valibot';

const searchSchema = v.object({
	page: v.optional(v.number())
});

const PAGE_SIZE = 20;

export const Route = new FileRoute('/_app/functions/$id').createRoute({
	component: Page,
	validateSearch: (d) => v.parse(searchSchema, d),
	loader: ({ context, params }) =>
		context.queryClient.ensureQueryData(getFunctionQueryOptions(params.id))
});

function Page() {
	const { id } = Route.useParams();
	const { page } = Route.useSearch();
	const { data } = useSuspenseQuery(getFunctionQueryOptions(id));

	const url = `${window.location.protocol}//${window.location.host}/invoke/functions/${
		data.function.endpoint || data.function.id
	}`;

	useQuery(listRevisionsQueryOptions(id, page, PAGE_SIZE));
	return (
		<div className="mx-auto max-w-[512px] gap-3 flex flex-col">
			<Card>
				<CardHeader>
					<div className="flex flex-col">
						<p className="text-md">{data.function.name}</p>
						<p className="text-small text-default-500">{data.function.id}</p>
					</div>
				</CardHeader>
				<Divider />
				<CardBody>
					<Code className="whitespace-pre-wrap break-all">curl -X POST {url}</Code>
				</CardBody>
			</Card>
			<Button as={Link} href={`/functions/${id}/edit`}>
				Edit
			</Button>
		</div>
	);
}
