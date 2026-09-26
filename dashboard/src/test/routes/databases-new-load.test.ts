import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects, getDatabaseCatalog } = vi.hoisted(() => ({
	listProjects: vi.fn(),
	getDatabaseCatalog: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listProjects,
	getDatabaseCatalog
}));

describe('databases/new page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		getDatabaseCatalog.mockReset();
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
		getDatabaseCatalog.mockResolvedValue([{ db_type: 'postgres', variant: '' }]);
	});

	it('loads database wizard route data before render', async () => {
		const pageModule = await import('../../routes/databases/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			catalog: [{ db_type: 'postgres', variant: '' }]
		});
	});

	it('still loads without the catalog (the form falls back to defaults)', async () => {
		getDatabaseCatalog.mockRejectedValue(new Error('offline'));
		const pageModule = await import('../../routes/databases/new/+page');
		await expect(pageModule.load()).resolves.toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			catalog: []
		});
	});
});
