import { getAlertRule, listApps } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [rule, apps] = await Promise.all([getAlertRule(params.id), listApps()]);

	return { rule, apps };
}
