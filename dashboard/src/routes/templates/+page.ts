import { listTemplates } from '$lib/api';

export async function load({ url }: { url: URL }) {
	return {
		catalog: await listTemplates(),
		category: url.searchParams.get('category') ?? ''
	};
}
