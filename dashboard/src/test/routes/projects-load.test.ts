import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects, listApps } = vi.hoisted(() => ({
	listProjects: vi.fn(),
	listApps: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listProjects,
	listApps
}));

describe('projects page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		listApps.mockReset();
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
	});

	it('loads projects route data before render', async () => {
		const pageModule = await import('../../routes/projects/+page');

		await expect(pageModule.load()).resolves.toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			apps: [{ id: 'app_1', name: 'api-service' }]
		});
	});
});
