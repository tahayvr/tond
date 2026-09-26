import { listApps, listIntegrations } from '$lib/api';

export async function load() {
	const [apps, integrations] = await Promise.all([listApps(), listIntegrations()]);
	return { apps, integrations };
}
