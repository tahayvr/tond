import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects } = vi.hoisted(() => ({
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listProjects
}));

describe('databases/new page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
	});

	it('loads database wizard route data before render', async () => {
		const pageModule = await import('../../routes/databases/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }]
		});
	});
});
