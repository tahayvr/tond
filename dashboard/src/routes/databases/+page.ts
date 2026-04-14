import { listDatabases } from '$lib/api';

export async function load() {
	return {
		databases: await listDatabases()
	};
}
