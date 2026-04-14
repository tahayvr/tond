import { listGitHubApps, listRegistryCredentials } from '$lib/api';

export async function load({ params }: { params: { source: string } }) {
	if (params.source === 'github') {
		return {
			sourceData: {
				githubApps: await listGitHubApps()
			}
		};
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
