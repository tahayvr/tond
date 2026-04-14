import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getDatabase, listProjects } = vi.hoisted(() => ({
	getDatabase: vi.fn(),
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getDatabase,
	listProjects
}));

describe('databases/[id] page load', () => {
	beforeEach(() => {
		getDatabase.mockReset();
		listProjects.mockReset();
		getDatabase.mockResolvedValue({ id: 'db_1', name: 'postgres' });
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
	});

	it('loads database detail route data before render', async () => {
		const pageModule = await import('../../routes/databases/[id]/+page');

		await expect(pageModule.load({ params: { id: 'db_1' } } as never)).resolves.toEqual({
			db: { id: 'db_1', name: 'postgres' },
			projects: [{ id: 'proj_1', name: 'Platform' }]
		});
	});
});
