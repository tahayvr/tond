import { getSettings, listDnsProviders } from '$lib/api';

export async function load() {
	const [providers, settings] = await Promise.all([
		listDnsProviders(),
		getSettings().catch(() => null)
	]);
	return {
		providers,
		appsDomain: settings?.apps_domain ?? '',
		previewDomain: settings?.preview_domain ?? ''
	};
}
