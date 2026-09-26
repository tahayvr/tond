import { getProject, getSettings, listApps, listDatabases, listProjectPreviews } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [project, allDatabases, allApps, projectPreviews, settings] = await Promise.all([
		getProject(params.id),
		listDatabases(),
		listApps(),
		listProjectPreviews(params.id),
		getSettings()
	]);

	return {
		project,
		allDatabases,
		allApps,
		projectPreviews,
		previewDomain: settings.preview_domain ?? ''
	};
}
