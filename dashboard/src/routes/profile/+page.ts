import { getMe, listSessions } from '$lib/api';

export async function load() {
	// Fetch the user fresh so 2FA state reflects changes made in other sessions.
	const [sessions, me] = await Promise.all([listSessions(), getMe()]);
	return { sessions, me };
}
