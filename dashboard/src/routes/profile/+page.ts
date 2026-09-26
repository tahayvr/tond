import { getMe, listSessions, listTokens } from '$lib/api';

export async function load() {
	// Fetch the user fresh so 2FA state reflects changes made in other sessions.
	const [sessions, me, tokens] = await Promise.all([listSessions(), getMe(), listTokens()]);
	return { sessions, me, tokens };
}
