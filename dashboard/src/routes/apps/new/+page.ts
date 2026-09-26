import { listProjects, listRegistryCredentials } from '$lib/api';

export async function load() {
	const [projects, registryCredentials] = await Promise.all([
		listProjects(),
		listRegistryCredentials()
	]);

	return {
		projects,
		registryCredentials
	};
}
