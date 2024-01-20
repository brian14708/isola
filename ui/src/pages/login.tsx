import { Button } from '@nextui-org/react';
import { FileRoute } from '@tanstack/react-router';
import * as v from 'valibot';

const searchSchema = v.object({
	redirect: v.optional(v.string())
});

export const Route = new FileRoute('/login').createRoute({
	component: Login,
	validateSearch: (d) => v.parse(searchSchema, d)
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
