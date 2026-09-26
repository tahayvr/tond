import { getAlertSettings, listAlertKinds, listApps, listIntegrations } from '$lib/api';

export async function load() {
	const [kinds, apps, integrations, settings] = await Promise.all([
		listAlertKinds(),
		listApps(),
		listIntegrations(),
		getAlertSettings().catch(() => null)
	]);
	return { kinds, apps, integrations, quietHours: settings?.quiet_hours ?? null };
}
