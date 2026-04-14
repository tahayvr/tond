import { listSnapshots } from '$lib/api';

export async function load() {
	return {
		snapshots: await listSnapshots()
	};
}
