import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listDatabases } = vi.hoisted(() => ({
	listDatabases: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listDatabases
}));

describe('databases page load', () => {
	beforeEach(() => {
		listDatabases.mockReset();
		listDatabases.mockResolvedValue([{ id: 'db_1', name: 'postgres' }]);
	});

	it('loads databases route data before render', async () => {
		const pageModule = await import('../../routes/databases/+page');

		await expect(pageModule.load()).resolves.toEqual({
			databases: [{ id: 'db_1', name: 'postgres' }]
		});
	});
});
