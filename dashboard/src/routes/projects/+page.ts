import { listApps, listProjects } from '$lib/api';

export async function load() {
	const [projects, apps] = await Promise.all([listProjects(), listApps()]);

	return { projects, apps };
}
