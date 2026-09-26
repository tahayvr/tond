import { error } from '@sveltejs/kit';
import { listProjects, listTemplates, type Project } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [catalog, projects] = await Promise.all([
		listTemplates(),
		// Only for the optional project picker: the page works without it.
		listProjects().catch((): Project[] => [])
	]);
	const template = catalog.templates.find((t) => t.id === params.id);
	if (!template) {
		error(404, `There is no template named ${params.id}.`);
	}
	return {
		template,
		memory: catalog.memory,
		appsDomain: catalog.apps_domain,
		projects
	};
}
