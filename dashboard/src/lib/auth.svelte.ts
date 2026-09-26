import { getAuthStatus, setUnauthorizedHandler, type AuthStatus, type User } from './api';

// Auth state -- module-level singletons using Svelte 5 runes via $state
// These are reactive and can be imported by any component.

// There is no 'setup' state: the server doesn't tell unauthenticated callers
// whether first-run setup is open. The /setup page is reached through the
// one-time link printed by the installer and checks its token itself.
let _status = $state<'loading' | 'login' | 'authenticated'>('loading');
let _user = $state<User | null>(null);
let _sessionExpired = $state(false);

export function getStatus() {
	return _status;
}

export function getUser() {
	return _user;
}

/** True when the user was signed out because their session expired (shown on /login). */
export function getSessionExpired() {
	return _sessionExpired;
}

export function setAuthenticated(user: User) {
	_user = user;
	_status = 'authenticated';
	_sessionExpired = false;
}

export function clearAuth() {
	_user = null;
	_status = 'login';
}

/**
 * Called by the API layer when an authenticated request gets a 401. The root
 * layout then redirects to /login?next=<current page>.
 */
export function handleSessionExpired() {
	if (_status !== 'authenticated') return;
	_sessionExpired = true;
	clearAuth();
}

setUnauthorizedHandler(handleSessionExpired);

export async function checkAuth(): Promise<void> {
	// Dev bypass: set VITE_DEV_AUTH_BYPASS=true in .env.local to skip the API check
	if (import.meta.env.DEV && import.meta.env.VITE_DEV_AUTH_BYPASS === 'true') {
		_status = 'authenticated';
		_user = {
			id: 'dev',
			email: 'dev@localhost',
			role: 'admin',
			totp_enabled: false,
			created_at: '1970-01-01T00:00:00.000Z'
		};
		return;
	}

	try {
		const authStatus: AuthStatus = await getAuthStatus();

		if (authStatus.authenticated && authStatus.user) {
			_status = 'authenticated';
			_user = authStatus.user;
		} else {
			_status = 'login';
			_user = null;
		}
	} catch {
		_status = 'login';
		_user = null;
	}
}
