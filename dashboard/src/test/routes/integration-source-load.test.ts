import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listGitHubApps, listRegistryCredentials } = vi.hoisted(() => ({
	listGitHubApps: vi.fn(),
	listRegistryCredentials: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listGitHubApps,
	listRegistryCredentials
}));

describe('integrations/[source] page load', () => {
	beforeEach(() => {
		listGitHubApps.mockReset();
		listRegistryCredentials.mockReset();
		listGitHubApps.mockResolvedValue([{ id: 'gh_1', name: 'Tond GitHub App' }]);
		listRegistryCredentials.mockResolvedValue([{ id: 'cred_1', name: 'GHCR' }]);
	});

	it('loads github app data before render', async () => {
		const pageModule = await import('../../routes/integrations/[source]/+page');

		await expect(pageModule.load({ params: { source: 'github' } } as never)).resolves.toEqual({
			sourceData: {
				githubApps: [{ id: 'gh_1', name: 'Tond GitHub App' }]
			}
		});
	});

	it('loads registry credentials before render', async () => {
		const pageModule = await import('../../routes/integrations/[source]/+page');

		await expect(pageModule.load({ params: { source: 'registry' } } as never)).resolves.toEqual({
			sourceData: {
				credentials: [{ id: 'cred_1', name: 'GHCR' }]
			}
		});
	});

	it('returns null source data for unknown integrations', async () => {
		const pageModule = await import('../../routes/integrations/[source]/+page');

		await expect(pageModule.load({ params: { source: 'unknown' } } as never)).resolves.toEqual({
			sourceData: null
		});
	});
});
