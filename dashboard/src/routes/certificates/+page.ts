import { listCertificates } from '$lib/api';

export async function load({ url }: { url: URL }) {
	return {
		certificates: await listCertificates(),
		problemsOnly: url.searchParams.get('filter') === 'problems'
	};
}
