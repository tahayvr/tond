import { getAuthStatus, type AuthStatus, type User } from './api';

// Auth state -- module-level singletons using Svelte 5 runes via $state
// These are reactive and can be imported by any component.

let _status = $state<'loading' | 'setup' | 'login' | 'authenticated'>('loading');
let _user = $state<User | null>(null);

export function getStatus() {
	return _status;
}

export function getUser() {
	return _user;
}

export function setAuthenticated(user: User) {
	_user = user;
	_status = 'authenticated';
}

export function clearAuth() {
	_user = null;
	_status = 'login';
}

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

		if (authStatus.needs_setup) {
			_status = 'setup';
			_user = null;
		} else if (authStatus.authenticated && authStatus.user) {
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
