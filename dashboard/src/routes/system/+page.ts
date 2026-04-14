import { getSystemMetrics } from '$lib/api';

export async function load() {
	return {
		metrics: await getSystemMetrics()
	};
}
