import {
	Button,
	Link,
	Table,
	TableHeader,
	TableColumn,
	TableBody,
	TableRow,
	TableCell,
	Pagination
} from '@nextui-org/react';
import { FileRoute, useNavigate } from '@tanstack/react-router';
import { FiPlus } from 'react-icons/fi';
import * as v from 'valibot';

import { listFunctionsQueryOptions } from '@/api/functions';
import { useSuspenseQuery } from '@tanstack/react-query';

const searchSchema = v.object({
	page: v.optional(v.number())
});

const PAGE_SIZE = 20;

export const Route = new FileRoute('/_app/functions/').createRoute({
	validateSearch: (d) => v.parse(searchSchema, d),
	loaderDeps: ({ search: { page } }) => ({ page }),
	component: Page,
	loader: (opts) =>
		opts.context.queryClient.ensureQueryData(listFunctionsQueryOptions(opts.deps.page, PAGE_SIZE))
});

function Page() {
	const { page } = Route.useLoaderDeps();
	const navigate = useNavigate();
	const { data } = useSuspenseQuery(listFunctionsQueryOptions(page, PAGE_SIZE));

	return (
		<div className="w-full items-center max-w-[640px] m-auto gap-5 flex flex-col">
			<Button as={Link} href="/functions/new" startContent={<FiPlus />} color="primary">
				Create Function
			</Button>

			<Table
				classNames={{
					table: 'min-h-[200px]'
				}}
				aria-label="Function List"
				bottomContent={
					data &&
					data.total > PAGE_SIZE && (
						<div className="flex w-full justify-center">
							<Pagination
								total={Math.ceil((data?.total ?? 0) / PAGE_SIZE)}
								page={page}
								showControls={true}
								onChange={(p) => {
									navigate({
										search: {
											page: p
										}
									});
								}}
							/>
						</div>
					)
				}
				selectionMode="single"
				onRowAction={(key) => {
					navigate({
						to: `/functions/${key}`
					});
				}}
			>
				<TableHeader>
					<TableColumn>Name</TableColumn>
				</TableHeader>
				<TableBody items={data?.functions || []}>
					{(item) => (
						<TableRow key={item.id}>
							<TableCell>{item.name}</TableCell>
						</TableRow>
					)}
				</TableBody>
			</Table>
		</div>
	);
}
