import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listSessions, getMe, listTokens } = vi.hoisted(() => ({
	listSessions: vi.fn(),
	getMe: vi.fn(),
	listTokens: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listSessions,
	getMe,
	listTokens
}));

describe('profile page load', () => {
	beforeEach(() => {
		listSessions.mockReset();
		listSessions.mockResolvedValue([{ id: 'sess_1', is_current: true }]);
		getMe.mockReset();
		getMe.mockResolvedValue({ id: 'usr_1', totp_enabled: true });
		listTokens.mockReset();
		listTokens.mockResolvedValue([{ id: 'tok_1', name: 'ci' }]);
	});

	it('loads sessions, API tokens and the current user (with 2FA state) before render', async () => {
		const pageModule = await import('../../routes/profile/+page');

		await expect(pageModule.load()).resolves.toEqual({
			sessions: [{ id: 'sess_1', is_current: true }],
			me: { id: 'usr_1', totp_enabled: true },
			tokens: [{ id: 'tok_1', name: 'ci' }]
		});
	});
});
