import {
	getSettings,
	listApps,
	listAuditLog,
	listCronJobs,
	listDatabases,
	listProjects
} from '$lib/api';

export async function load() {
	const [projects, apps, databases, cronJobs, activity, settings] = await Promise.all([
		listProjects(),
		listApps(),
		listDatabases(),
		listCronJobs(),
		listAuditLog({ limit: 5 }),
		getSettings()
	]);
	const recentActivity = activity.entries;

	return {
		projects,
		apps,
		databases,
		cronJobs,
		recentActivity,
		serverName: settings.server_name
	};
}
