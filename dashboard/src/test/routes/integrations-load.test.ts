import { beforeEach, describe, expect, it, vi } from 'vitest';

const {
	listGitHubApps,
	listRegistryCredentials,
	listIntegrations,
	listGitLabTokens,
	listGiteaTokens,
	listDnsProviders
} = vi.hoisted(() => ({
	listGitHubApps: vi.fn(),
	listRegistryCredentials: vi.fn(),
	listIntegrations: vi.fn(),
	listGitLabTokens: vi.fn(),
	listGiteaTokens: vi.fn(),
	listDnsProviders: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listGitHubApps,
	listRegistryCredentials,
	listIntegrations,
	listGitLabTokens,
	listGiteaTokens,
	listDnsProviders
}));

describe('integrations page load', () => {
	beforeEach(() => {
		listGitHubApps.mockReset();
		listRegistryCredentials.mockReset();
		listIntegrations.mockReset();
		listGitHubApps.mockResolvedValue([{ id: 'gh_1', name: 'Tond GitHub App' }]);
		listRegistryCredentials.mockResolvedValue([{ id: 'cred_1', name: 'GHCR' }]);
		listIntegrations.mockResolvedValue([{ id: 'int_1', kind: 'telegram', enabled: true }]);
		listGitLabTokens.mockResolvedValue([]);
		listGiteaTokens.mockRejectedValue(new Error('offline'));
		listDnsProviders.mockResolvedValue([{ id: 'dns_1', name: 'Cloudflare' }]);
	});

	it('loads integration summary route data before render', async () => {
		const pageModule = await import('../../routes/integrations/+page');

		await expect(pageModule.load()).resolves.toEqual({
			githubApps: [{ id: 'gh_1', name: 'Tond GitHub App' }],
			credentials: [{ id: 'cred_1', name: 'GHCR' }],
			integrations: [{ id: 'int_1', kind: 'telegram', enabled: true }],
			gitlabTokens: [],
			giteaTokens: [],
			dnsProviders: [{ id: 'dns_1', name: 'Cloudflare' }]
		});
	});
});
