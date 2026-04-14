import { listApps, listProjects } from '$lib/api';

export async function load() {
	const [apps, projects] = await Promise.all([listApps(), listProjects()]);

	return { apps, projects };
}
