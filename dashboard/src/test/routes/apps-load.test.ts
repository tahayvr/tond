import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listApps, listProjects } = vi.hoisted(() => ({
	listApps: vi.fn(),
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listApps,
	listProjects
}));

describe('apps page load', () => {
	beforeEach(() => {
		listApps.mockReset();
		listProjects.mockReset();
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
	});

	it('loads apps route data before render', async () => {
		const pageModule = await import('../../routes/apps/+page');

		await expect(pageModule.load()).resolves.toEqual({
			apps: [{ id: 'app_1', name: 'api-service' }],
			projects: [{ id: 'proj_1', name: 'Platform' }]
		});
	});
});
