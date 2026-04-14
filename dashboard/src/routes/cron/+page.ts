import { listCronJobs } from '$lib/api';

export async function load() {
	return {
		jobs: await listCronJobs()
	};
}
