import { listCertificates, listDnsProviders } from '$lib/api';

export async function load({ url }: { url: URL }) {
	const [certificates, dnsProviders] = await Promise.all([
		listCertificates(),
		listDnsProviders().catch(() => [])
	]);
	return {
		certificates,
		dnsProviders,
		problemsOnly: url.searchParams.get('filter') === 'problems'
	};
}
