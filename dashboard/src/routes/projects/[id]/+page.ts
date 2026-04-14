import { getProject, listApps, listDatabases, listProjectPreviews } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [project, allDatabases, allApps, projectPreviews] = await Promise.all([
		getProject(params.id),
		listDatabases(),
		listApps(),
		listProjectPreviews(params.id)
	]);

	return { project, allDatabases, allApps, projectPreviews };
}
