import {
	listGitHubApps,
	listGitLabTokens,
	listGiteaTokens,
	listIntegrations,
	listRegistryCredentials,
	type GitLabToken,
	type GiteaToken
} from '$lib/api';

export async function load() {
	const [githubApps, credentials, integrations, gitlabTokens, giteaTokens] = await Promise.all([
		listGitHubApps(),
		listRegistryCredentials(),
		listIntegrations(),
		listGitLabTokens().catch((): GitLabToken[] => []),
		listGiteaTokens().catch((): GiteaToken[] => [])
	]);

	return { githubApps, credentials, integrations, gitlabTokens, giteaTokens };
}
