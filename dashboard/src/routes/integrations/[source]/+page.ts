import { listGitHubApps, listGitLabTokens, listGiteaTokens, listRegistryCredentials } from '$lib/api';

export async function load({ params }: { params: { source: string } }) {
	if (params.source === 'github') {
		return {
			sourceData: {
				githubApps: await listGitHubApps()
			}
		};
	}

	if (params.source === 'gitlab') {
		return { sourceData: { gitlabTokens: await listGitLabTokens() } };
	}

	if (params.source === 'gitea') {
		return { sourceData: { giteaTokens: await listGiteaTokens() } };
	}

	if (params.source === 'registry') {
		return {
			sourceData: {
				credentials: await listRegistryCredentials()
			}
		};
	}

	return {
		sourceData: null
	};
}
