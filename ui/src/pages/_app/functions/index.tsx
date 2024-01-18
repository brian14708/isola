import { useFunctionList } from '@/api/functions';
import {
	Button,
	Link,
	Table,
	TableHeader,
	TableColumn,
	TableBody,
	TableRow,
	TableCell,
	Pagination,
	Spinner
} from '@nextui-org/react';

import { FileRoute, useNavigate } from '@tanstack/react-router';
import { FiGlobe, FiLock, FiPlus, FiShield } from 'react-icons/fi';
import { z } from 'zod';

const searchSchema = z.object({
	page: z.number().optional()
});

export const Route = new FileRoute('/_app/functions/').createRoute({
	component: Page,
	validateSearch: searchSchema.parse
});

const ICON_MAP = {
	public: <FiGlobe className="text-gray-500" />,
	internal: <FiShield className="text-gray-500" />,
	private: <FiLock className="text-gray-500" />
} as const;
const PAGE_SIZE = 15;

function Page() {
	const { page } = Route.useSearch();
	const navigate = useNavigate();
	const { data, isLoading } = useFunctionList(page ?? 1, PAGE_SIZE);

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
			>
				<TableHeader>
					<TableColumn>Name</TableColumn>
				</TableHeader>
				<TableBody items={data?.functions || []} isLoading={isLoading} loadingContent={<Spinner />}>
					{(item) => (
						<TableRow key={item.id}>
							<TableCell className="flex items-center gap-2">
								{ICON_MAP[item.visibility]}
								{item.name}
							</TableCell>
						</TableRow>
					)}
				</TableBody>
			</Table>
		</div>
	);
}
