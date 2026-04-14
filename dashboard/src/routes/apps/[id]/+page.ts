import { getApp } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	return {
		app: await getApp(params.id)
	};
}
