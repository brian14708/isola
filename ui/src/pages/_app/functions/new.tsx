import { Button, Input, Radio, RadioGroup } from '@nextui-org/react';
import { FileRoute, useNavigate } from '@tanstack/react-router';
import { toast } from 'react-toastify';
import { useForm, Controller } from 'react-hook-form';
import { useMutation, useQueryClient } from '@tanstack/react-query';

export const Route = new FileRoute('/_app/functions/new').createRoute({
	component: Page
});

type Inputs = {
	name: string;
	endpoint: string | null;
	visibility: 'public' | 'private' | 'internal';
};

function Page() {
	const navigate = useNavigate();
	const { register, handleSubmit, control } = useForm<Inputs>();
	const queryClient = useQueryClient();
	const { mutate } = useMutation({
		mutationFn: async (data: Inputs) => {
			const d = await fetch('/api/functions', {
				method: 'PUT',
				headers: {
					'Content-Type': 'application/json'
				},
				body: JSON.stringify(data)
			});
			if (d.status >= 400) {
				let msg = 'Unknown Error';
				try {
					msg = (await d.json()).message;
				} catch {
					// ignore
				}
				toast.error(`Error: ${msg}`);
				return;
			}
			queryClient.invalidateQueries({ queryKey: ['/functions'] });

			toast.success('Function created successfully!');
			navigate({
				to: '/functions'
			});
		}
	});

	return (
		<form
			onSubmit={handleSubmit((data) => mutate(data))}
			className="mx-auto w-full max-w-[640px] py-8 flex flex-col gap-4"
		>
			<h1 className="text-xl text-center my-4">Create Function</h1>
			<Input {...register('name', { required: true })} type="text" label="Name" isRequired />
			<Input
				{...register('endpoint', { setValueAs: (v) => v || null })}
				type="text"
				label="Endpoint"
			/>
			<Controller
				control={control}
				name="visibility"
				defaultValue="private"
				rules={{ required: true }}
				render={({ field: { value, onChange } }) => (
					<RadioGroup
						value={value}
						onValueChange={onChange}
						orientation="horizontal"
						label="Visibility"
						isRequired
					>
						<Radio value="private">Private</Radio>
						<Radio value="internal">Internal</Radio>
						<Radio value="public">Public</Radio>
					</RadioGroup>
				)}
			/>

			<Button type="submit">Create</Button>
		</form>
	);
}
