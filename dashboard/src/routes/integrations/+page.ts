import { listGitHubApps, listIntegrations, listRegistryCredentials } from '$lib/api';

export async function load() {
	const [githubApps, credentials, integrations] = await Promise.all([
		listGitHubApps(),
		listRegistryCredentials(),
		listIntegrations()
	]);

	return { githubApps, credentials, integrations };
}
