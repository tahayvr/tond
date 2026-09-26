import { listUsers } from '$lib/api';

export async function load() {
	return { users: await listUsers() };
}
