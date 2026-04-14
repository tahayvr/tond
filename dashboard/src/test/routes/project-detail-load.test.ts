import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getProject, listDatabases, listApps, listProjectPreviews } = vi.hoisted(() => ({
	getProject: vi.fn(),
	listDatabases: vi.fn(),
	listApps: vi.fn(),
	listProjectPreviews: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getProject,
	listDatabases,
	listApps,
	listProjectPreviews
}));

describe('projects/[id] page load', () => {
	beforeEach(() => {
		getProject.mockReset();
		listDatabases.mockReset();
		listApps.mockReset();
		listProjectPreviews.mockReset();
		getProject.mockResolvedValue({ id: 'proj_1', name: 'Platform', apps: [] });
		listDatabases.mockResolvedValue([{ id: 'db_1', name: 'postgres' }]);
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
		listProjectPreviews.mockResolvedValue([]);
	});

	it('loads project detail route data before render', async () => {
		const pageModule = await import('../../routes/projects/[id]/+page');

		await expect(pageModule.load({ params: { id: 'proj_1' } } as never)).resolves.toEqual({
			project: { id: 'proj_1', name: 'Platform', apps: [] },
			allDatabases: [{ id: 'db_1', name: 'postgres' }],
			allApps: [{ id: 'app_1', name: 'api-service' }],
			projectPreviews: []
		});
	});
});
