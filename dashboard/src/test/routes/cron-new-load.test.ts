import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects } = vi.hoisted(() => ({
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listProjects
}));

describe('cron/new page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
	});

	it('loads cron wizard route data before render', async () => {
		const pageModule = await import('../../routes/cron/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }]
		});
	});
});
