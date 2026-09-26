import {
	listDnsProviders,
	listGitHubApps,
	listIntegrations,
	listRegistryCredentials
} from '$lib/api';

export async function load() {
	const [githubApps, credentials, integrations, dnsProviders] = await Promise.all([
		listGitHubApps(),
		listRegistryCredentials(),
		listIntegrations(),
		listDnsProviders().catch(() => [])
	]);

	return { githubApps, credentials, integrations, dnsProviders };
}
