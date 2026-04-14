import { listApps } from '$lib/api';

export async function load() {
	return {
		apps: await listApps()
	};
}
