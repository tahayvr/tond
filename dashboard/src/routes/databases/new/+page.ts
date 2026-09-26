import { getDatabaseCatalog, listProjects, type DbCatalogEntry } from '$lib/api';

export async function load() {
	// The catalog only adds choices (version, image); the form works without it.
	const [projects, catalog] = await Promise.all([
		listProjects(),
		getDatabaseCatalog().catch((): DbCatalogEntry[] => [])
	]);
	return { projects, catalog };
}
