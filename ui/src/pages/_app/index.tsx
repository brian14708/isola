import { useUserProfile } from '@/api/users';
import { FileRoute } from '@tanstack/react-router';

export const Route = new FileRoute('/_app/').createRoute({
	component: Home
});

function Home() {
	const { data } = useUserProfile();
	return (
		<div className="w-full justify-center flex">
			<h2 className="text-xl">👋 Welcome {data?.name}!</h2>
		</div>
	);
}
