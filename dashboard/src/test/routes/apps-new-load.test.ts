import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects, listRegistryCredentials } = vi.hoisted(() => ({
	listProjects: vi.fn(),
	listRegistryCredentials: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listProjects,
	listRegistryCredentials
}));

describe('apps/new page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		listRegistryCredentials.mockReset();

		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
		listRegistryCredentials.mockResolvedValue([{ id: 'cred_1', name: 'GHCR' }]);
	});

	it('loads route data before rendering the app wizard', async () => {
		const pageModule = await import('../../routes/apps/new/+page');

		const data = await pageModule.load();

		expect(data).toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			registryCredentials: [{ id: 'cred_1', name: 'GHCR' }]
		});
	});

	it('throws when wizard bootstrap data fails to load', async () => {
		const pageModule = await import('../../routes/apps/new/+page');
		listProjects.mockRejectedValue(new Error('Projects unavailable'));

		await expect(pageModule.load()).rejects.toThrow('Projects unavailable');
	});
});
