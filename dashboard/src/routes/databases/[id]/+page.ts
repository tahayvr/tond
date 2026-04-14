import { getDatabase, listProjects } from '$lib/api';

export async function load({ params }: { params: { id: string } }) {
	const [db, projects] = await Promise.all([getDatabase(params.id), listProjects()]);

	return { db, projects };
}
