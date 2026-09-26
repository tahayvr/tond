import { listActiveAlerts, listAlertRules, listApps, type App } from '$lib/api';

export async function load() {
	const [rules, active, apps] = await Promise.all([
		listAlertRules(),
		listActiveAlerts(),
		// Only used for the history filter.
		listApps().catch(() => [] as App[])
	]);
	return { rules, active, apps };
}
