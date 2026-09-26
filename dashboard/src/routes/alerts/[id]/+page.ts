import {
	getAlertRule,
	getAlertSettings,
	listActiveAlerts,
	listAlertKinds,
	listApps,
	listIntegrations,
	type App,
	type Integration
} from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [rule, active, kinds, apps, integrations, settings] = await Promise.all([
		getAlertRule(params.id),
		listActiveAlerts(),
		listAlertKinds(),
		listApps().catch(() => [] as App[]),
		// Channels need settings:write; viewers see the rule without them.
		listIntegrations().catch(() => [] as Integration[]),
		getAlertSettings().catch(() => null)
	]);
	return {
		rule,
		active: active.filter((a) => a.rule_id === params.id),
		kinds,
		apps,
		integrations,
		quietHours: settings?.quiet_hours ?? null
	};
}
