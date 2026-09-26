import {
	listDnsProviders,
	listGitHubApps,
	listGitLabTokens,
	listGiteaTokens,
	listIntegrations,
	listRegistryCredentials,
	type DnsProvider,
	type GitLabToken,
	type GiteaToken
} from '$lib/api';

export async function load() {
	const [githubApps, credentials, integrations, gitlabTokens, giteaTokens, dnsProviders] =
		await Promise.all([
			listGitHubApps(),
			listRegistryCredentials(),
			listIntegrations(),
			listGitLabTokens().catch((): GitLabToken[] => []),
			listGiteaTokens().catch((): GiteaToken[] => []),
			listDnsProviders().catch((): DnsProvider[] => [])
		]);

	return { githubApps, credentials, integrations, gitlabTokens, giteaTokens, dnsProviders };
}
