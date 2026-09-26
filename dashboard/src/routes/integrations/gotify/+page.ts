import { listIntegrations } from '$lib/api';

export async function load() {
	return {
		integrations: (await listIntegrations()).filter((integration) => integration.kind === 'gotify')
	};
}
