import { beforeEach, describe, expect, it, vi } from 'vitest';

const getAuthStatus = vi.fn();

vi.mock('./api', () => ({
	getAuthStatus,
	setUnauthorizedHandler: vi.fn()
}));

describe('auth state', () => {
	beforeEach(() => {
		getAuthStatus.mockReset();
		vi.stubEnv('VITE_DEV_AUTH_BYPASS', 'false');
	});

	it('sets authenticated state when auth status returns a user', async () => {
		getAuthStatus.mockResolvedValue({
			authenticated: true,
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				totp_enabled: false,
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const auth = await import('./auth.svelte');

		await auth.checkAuth();

		expect(auth.getStatus()).toBe('authenticated');
		expect(auth.getUser()).toMatchObject({ email: 'admin@example.com' });
	});

	it('sets login state for unauthenticated callers (setup state is never inferred)', async () => {
		getAuthStatus.mockResolvedValue({
			authenticated: false,
			user: null
		});

		const auth = await import('./auth.svelte');

		auth.clearAuth();
		await auth.checkAuth();

		expect(auth.getStatus()).toBe('login');
		expect(auth.getUser()).toBeNull();
	});

	it('falls back to login state when auth check fails', async () => {
		getAuthStatus.mockRejectedValue(new Error('network down'));

		const auth = await import('./auth.svelte');

		auth.setAuthenticated({
			id: 'usr_1',
			email: 'admin@example.com',
			role: 'admin',
			totp_enabled: false,
			created_at: '2026-01-01T00:00:00.000Z'
		});
		await auth.checkAuth();

		expect(auth.getStatus()).toBe('login');
		expect(auth.getUser()).toBeNull();
	});

	it('uses dev auth bypass when enabled', async () => {
		vi.stubEnv('VITE_DEV_AUTH_BYPASS', 'true');

		const auth = await import('./auth.svelte');

		auth.clearAuth();
		await auth.checkAuth();

		expect(getAuthStatus).not.toHaveBeenCalled();
		expect(auth.getStatus()).toBe('authenticated');
		expect(auth.getUser()).toMatchObject({
			id: 'dev',
			email: 'dev@localhost',
			role: 'admin'
		});
	});

	it('supports explicit auth state changes', async () => {
		const auth = await import('./auth.svelte');

		auth.setAuthenticated({
			id: 'usr_2',
			email: 'member@example.com',
			role: 'viewer',
			totp_enabled: false,
			created_at: '2026-01-01T00:00:00.000Z'
		});

		expect(auth.getStatus()).toBe('authenticated');
		expect(auth.getUser()).toMatchObject({ email: 'member@example.com' });

		auth.clearAuth();

		expect(auth.getStatus()).toBe('login');
		expect(auth.getUser()).toBeNull();
	});

	it('signs out and flags an expired session only when a session was active', async () => {
		const auth = await import('./auth.svelte');

		auth.clearAuth();
		auth.handleSessionExpired();
		expect(auth.getSessionExpired()).toBe(false);

		auth.setAuthenticated({
			id: 'usr_1',
			email: 'admin@example.com',
			role: 'admin',
			totp_enabled: false,
			created_at: '2026-01-01T00:00:00.000Z'
		});
		auth.handleSessionExpired();

		expect(auth.getStatus()).toBe('login');
		expect(auth.getUser()).toBeNull();
		expect(auth.getSessionExpired()).toBe(true);
	});
});
