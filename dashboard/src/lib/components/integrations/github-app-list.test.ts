import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./github-app-card.svelte', async () => ({
	default: (await import('../../../test/mocks/GitHubAppCardHarness.svelte')).default
}));

const api = vi.hoisted(() => ({
	listGitHubApps: vi.fn(),
	getGitHubAppManifestUrl: vi.fn(),
	deleteGitHubApp: vi.fn(),
	getGitHubAppInstallUrl: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return {
		...actual,
		...api
	};
});

import GitHubAppList from './github-app-list.svelte';

describe('GitHubAppList', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.getGitHubAppManifestUrl.mockResolvedValue({
			github_url: 'https://github.com/settings/apps/new',
			manifest: '{"name":"tond"}'
		});
		api.deleteGitHubApp.mockResolvedValue(undefined);
	});

	it('renders route-provided apps without loading on mount', () => {
		render(GitHubAppList, {
			props: {
				initialApps: [
					{
						id: 'gh_1',
						name: 'Tond GitHub App',
						app_id: 123,
						client_id: 'Iv1.test',
						slug: 'tond-github-app',
						installation_id: 456,
						installed: true,
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				]
			}
		});

		expect(screen.queryByText(/loading/i)).not.toBeInTheDocument();
		expect(screen.getByText(/add another app/i)).toBeInTheDocument();
		expect(api.listGitHubApps).not.toHaveBeenCalled();
	});

	it('starts the github app connect flow with a manifest form submit', async () => {
		const submit = vi.fn();
		const originalCreateElement = document.createElement.bind(document);
		vi.spyOn(document, 'createElement').mockImplementation((tagName: string) => {
			const element = originalCreateElement(tagName);
			if (tagName === 'form') {
				(element as HTMLFormElement).submit = submit;
			}
			return element;
		});

		const user = userEvent.setup();
		render(GitHubAppList, { props: { initialApps: [] } });

		await user.click(screen.getByRole('button', { name: /create github app/i }));

		expect(api.getGitHubAppManifestUrl).toHaveBeenCalledOnce();
		expect(submit).toHaveBeenCalledOnce();
	});

	it('starts an install flow through the API instead of a static URL', async () => {
		api.getGitHubAppInstallUrl.mockResolvedValue('javascript:alert(1)');
		const user = userEvent.setup();
		render(GitHubAppList, {
			props: {
				initialApps: [
					{
						id: 'gh_1',
						name: 'Tond GitHub App',
						app_id: 123,
						client_id: 'Iv1.test',
						slug: 'tond-github-app',
						installation_id: null,
						installed: false,
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				]
			}
		});

		await user.click(screen.getByRole('button', { name: /install app/i }));

		expect(api.getGitHubAppInstallUrl).toHaveBeenCalledWith('gh_1');
		// A non-https URL is never navigated to.
		expect(await screen.findByText(/unexpected installation url/i)).toBeInTheDocument();
	});

	it('disconnects a github app from the list', async () => {
		const user = userEvent.setup();
		render(GitHubAppList, {
			props: {
				initialApps: [
					{
						id: 'gh_1',
						name: 'Tond GitHub App',
						app_id: 123,
						client_id: 'Iv1.test',
						slug: 'tond-github-app',
						installation_id: 456,
						installed: true,
						created_at: '2026-01-01T00:00:00.000Z',
						updated_at: '2026-01-01T00:00:00.000Z'
					}
				]
			}
		});

		await user.click(screen.getByRole('button', { name: /disconnect app/i }));

		expect(api.deleteGitHubApp).toHaveBeenCalledWith('gh_1');
		expect(screen.queryByText(/tond github app/i)).not.toBeInTheDocument();
	});
});
