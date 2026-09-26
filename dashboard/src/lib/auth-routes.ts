// Routes that are reachable without a session. The root layout uses this one
// list both to decide where to redirect and whether to render the app shell,
// so the two can never disagree (e.g. /login/2fa bouncing back to /login).

export const AUTH_ROUTES = ['/login', '/login/2fa', '/setup'] as const;

// There is no 'setup' status: /setup is reached through the installer's one-time
// link and validates its token itself, so signed-out visitors may stay on it.
export type AuthStatus = 'loading' | 'login' | 'authenticated';

export function isAuthRoute(pathname: string): boolean {
	const path = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
	return (AUTH_ROUTES as readonly string[]).includes(path);
}

/**
 * Validate a post-login `next` target. Only same-origin paths are accepted:
 * absolute URLs, protocol-relative `//host` and `/\host` tricks, and auth
 * routes (which would loop) all fall back to `/`.
 */
export function safeNextPath(next: string | null | undefined): string {
	if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\')) {
		return '/';
	}
	// eslint-disable-next-line no-control-regex
	if (/[\u0000-\u001f\\]/.test(next)) return '/';
	try {
		const base = 'http://tond.invalid';
		const url = new URL(next, base);
		if (url.origin !== base) return '/';
		if (isAuthRoute(url.pathname)) return '/';
		return `${url.pathname}${url.search}${url.hash}`;
	} catch {
		return '/';
	}
}

/** `/login`, carrying the page the user was on so they can return to it. */
export function loginUrl(from: URL | null): string {
	if (!from) return '/login';
	const next = safeNextPath(`${from.pathname}${from.search}${from.hash}`);
	return next === '/' ? '/login' : `/login?next=${encodeURIComponent(next)}`;
}

/**
 * Where the root layout should send the user for the current auth status, or
 * `null` to stay on the current page.
 */
export function authRedirect(status: AuthStatus, url: URL): string | null {
	const path = url.pathname;
	switch (status) {
		case 'loading':
			return null;
		case 'login':
			return isAuthRoute(path) ? null : loginUrl(url);
		case 'authenticated':
			return isAuthRoute(path) ? safeNextPath(url.searchParams.get('next')) : null;
	}
}

/** Whether the full dashboard shell (header, feed, footer) may render. */
export function showsAppShell(status: AuthStatus, pathname: string): boolean {
	return status === 'authenticated' && !isAuthRoute(pathname);
}
