import { beforeEach, describe, expect, it, vi } from 'vitest';

const getAuthStatus = vi.fn();

vi.mock('./api', () => ({
	getAuthStatus
}));

describe('auth state', () => {
	beforeEach(() => {
		getAuthStatus.mockReset();
		vi.stubEnv('VITE_DEV_AUTH_BYPASS', 'false');
	});

	it('sets authenticated state when auth status returns a user', async () => {
		getAuthStatus.mockResolvedValue({
			needs_setup: false,
			authenticated: true,
			user: {
				id: 'usr_1',
				email: 'admin@example.com',
				role: 'admin',
				created_at: '2026-01-01T00:00:00.000Z'
			}
		});

		const auth = await import('./auth.svelte');

		await auth.checkAuth();

		expect(auth.getStatus()).toBe('authenticated');
		expect(auth.getUser()).toMatchObject({ email: 'admin@example.com' });
	});

	it('sets setup state when initial admin setup is required', async () => {
		getAuthStatus.mockResolvedValue({
			needs_setup: true,
			authenticated: false,
			user: null
		});

		const auth = await import('./auth.svelte');

		auth.clearAuth();
		await auth.checkAuth();

		expect(auth.getStatus()).toBe('setup');
		expect(auth.getUser()).toBeNull();
	});

	it('falls back to login state when auth check fails', async () => {
		getAuthStatus.mockRejectedValue(new Error('network down'));

		const auth = await import('./auth.svelte');

		auth.setAuthenticated({
			id: 'usr_1',
			email: 'admin@example.com',
			role: 'admin',
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
			role: 'member',
			created_at: '2026-01-01T00:00:00.000Z'
		});

		expect(auth.getStatus()).toBe('authenticated');
		expect(auth.getUser()).toMatchObject({ email: 'member@example.com' });

		auth.clearAuth();

		expect(auth.getStatus()).toBe('login');
		expect(auth.getUser()).toBeNull();
	});
});
