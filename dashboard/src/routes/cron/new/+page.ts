import { listProjects } from '$lib/api';

export async function load() {
	return {
		projects: await listProjects()
	};
}
