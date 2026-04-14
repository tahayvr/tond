import { listApps, listAuditLog } from '$lib/api';

export async function load() {
	const initialFilterAppId = '';
	const initialFilterAction = '';
	const initialFilterLimit = 50;

	const [entries, apps] = await Promise.all([
		listAuditLog({ app_id: undefined, limit: initialFilterLimit }),
		listApps()
	]);

	return {
		entries,
		apps,
		initialFilterAppId,
		initialFilterAction,
		initialFilterLimit
	};
}
