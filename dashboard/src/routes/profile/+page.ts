import { listSessions } from '$lib/api';

export async function load() {
	return {
		sessions: await listSessions()
	};
}
