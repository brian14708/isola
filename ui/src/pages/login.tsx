import { Button } from '@nextui-org/react';
import { FileRoute } from '@tanstack/react-router';
import { z } from 'zod';

const searchSchema = z.object({
	redirect: z.string().optional()
});

export const Route = new FileRoute('/login').createRoute({
	component: Login,
	validateSearch: searchSchema.parse
});

function Login() {
	const { redirect } = Route.useSearch();

	return (
		<div className="p-2 flex items-center justify-center w-full">
			<a href={`/api/user/login${redirect ? `?redirect=${encodeURIComponent(redirect)}` : ''}`}>
				<Button>Login</Button>
			</a>
		</div>
	);
}
