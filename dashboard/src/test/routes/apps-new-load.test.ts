import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listTemplates, listProjects, listRegistryCredentials } = vi.hoisted(() => ({
	listTemplates: vi.fn(),
	listProjects: vi.fn(),
	listRegistryCredentials: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listTemplates,
	listProjects,
	listRegistryCredentials
}));

describe('apps/new page load', () => {
	beforeEach(() => {
		listTemplates.mockReset();
		listProjects.mockReset();
		listRegistryCredentials.mockReset();

		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
		listRegistryCredentials.mockResolvedValue([{ id: 'cred_1', name: 'GHCR' }]);
		listTemplates.mockResolvedValue([{ id: 'tmpl_1', name: 'Template One' }]);
	});

	it('loads route data before rendering the app wizard', async () => {
		const pageModule = await import('../../routes/apps/new/+page');

		const data = await pageModule.load();

		expect(data).toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			registryCredentials: [{ id: 'cred_1', name: 'GHCR' }],
			templates: [{ id: 'tmpl_1', name: 'Template One' }]
		});
	});

	it('throws when wizard bootstrap data fails to load', async () => {
		const pageModule = await import('../../routes/apps/new/+page');
		listTemplates.mockRejectedValue(new Error('Templates unavailable'));

		await expect(pageModule.load()).rejects.toThrow('Templates unavailable');
	});
});
