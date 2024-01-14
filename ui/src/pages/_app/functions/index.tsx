import { useListFunctions } from '@/api/functions';
import {
	Button,
	Link,
	Table,
	TableHeader,
	TableColumn,
	TableBody,
	TableRow,
	TableCell,
	getKeyValue
} from '@nextui-org/react';

import { FileRoute } from '@tanstack/react-router';
import { FiPlus } from 'react-icons/fi';

export const Route = new FileRoute('/_app/functions/').createRoute({
	component: Page
});

function Page() {
	const { data } = useListFunctions(0);
	return (
		<div className="w-full items-center gap-3 flex flex-col">
			<div>
				<Button as={Link} href="/functions/new" startContent={<FiPlus />} color="primary">
					Create Function
				</Button>
			</div>

			<Table aria-label="Example static collection table">
				<TableHeader
					columns={[
						{ key: 'name', title: 'NAME' },
						{ key: 'visibility', title: 'ROLE' },
						{ key: 'status', title: 'STATUS' }
					]}
				>
					{(column) => <TableColumn key={column.key}>{column.title}</TableColumn>}
				</TableHeader>
				<TableBody items={data?.functions || []}>
					{(item) => (
						<TableRow key={item.id}>
							{(columnKey) => {
								switch (columnKey) {
									case 'visibility':
										return <TableCell>{item.visibility.toUpperCase()}</TableCell>;
									default:
										return <TableCell>{getKeyValue(item, columnKey)}</TableCell>;
								}
							}}
						</TableRow>
					)}
				</TableBody>
			</Table>
		</div>
	);
}
