import { listProjects, listRegistryCredentials, listTemplates } from '$lib/api';

export async function load() {
	const [projects, registryCredentials, templates] = await Promise.all([
		listProjects(),
		listRegistryCredentials(),
		listTemplates()
	]);

	return {
		projects,
		registryCredentials,
		templates
	};
}
