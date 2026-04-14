import { getCronJob, listProjects } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [job, projects] = await Promise.all([getCronJob(params.id), listProjects()]);

	return { job, projects };
}
