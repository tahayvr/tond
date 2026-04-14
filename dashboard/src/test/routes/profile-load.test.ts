import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listSessions } = vi.hoisted(() => ({
	listSessions: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listSessions
}));

describe('profile page load', () => {
	beforeEach(() => {
		listSessions.mockReset();
		listSessions.mockResolvedValue([{ id: 'sess_1', is_current: true }]);
	});

	it('loads profile session data before render', async () => {
		const pageModule = await import('../../routes/profile/+page');

		await expect(pageModule.load()).resolves.toEqual({
			sessions: [{ id: 'sess_1', is_current: true }]
		});
	});
});
