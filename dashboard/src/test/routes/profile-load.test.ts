import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listSessions, getMe } = vi.hoisted(() => ({
	listSessions: vi.fn(),
	getMe: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listSessions,
	getMe
}));

describe('profile page load', () => {
	beforeEach(() => {
		listSessions.mockReset();
		listSessions.mockResolvedValue([{ id: 'sess_1', is_current: true }]);
		getMe.mockReset();
		getMe.mockResolvedValue({ id: 'usr_1', totp_enabled: true });
	});

	it('loads sessions and the current user (with 2FA state) before render', async () => {
		const pageModule = await import('../../routes/profile/+page');

		await expect(pageModule.load()).resolves.toEqual({
			sessions: [{ id: 'sess_1', is_current: true }],
			me: { id: 'usr_1', totp_enabled: true }
		});
	});
});
