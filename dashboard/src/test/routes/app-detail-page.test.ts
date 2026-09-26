import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { App } from '$lib/api';

const state = vi.hoisted(() => ({
	page: {
		url: new URL('http://localhost/apps/app_1'),
		params: { id: 'app_1' }
	}
}));

const { goto, setUiContext, clearUiContext, appContext, openFeedPanel } = vi.hoisted(() => ({
	goto: vi.fn(),
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	appContext: vi.fn((app) => ({ type: 'app', app, feed: { title: app.name, wsUrl: null } })),
	openFeedPanel: vi.fn()
}));

const api = vi.hoisted(() => ({
	deployApp: vi.fn(),
	stopApp: vi.fn(),
	startApp: vi.fn(),
	restartApp: vi.fn(),
	deleteApp: vi.fn(),
	updateDomain: vi.fn(),
	updateHealth: vi.fn(),
	updateDrainTimeout: vi.fn(),
	updateAppResources: vi.fn(),
	addVolume: vi.fn(),
	removeVolume: vi.fn(),
	listDeployments: vi.fn(),
	getDeploymentGeneratedDockerfile: vi.fn(),
	rollbackApp: vi.fn(),
	generateDeployKey: vi.fn(),
	getDeployKey: vi.fn(),
	setWebhookSecret: vi.fn(),
	getWebhookConfig: vi.fn(),
	rotateWebhookSecret: vi.fn(),
	revealWebhookSecret: vi.fn(),
	setAutoDeploy: vi.fn(),
	updateSource: vi.fn(),
	listPreviews: vi.fn(),
	getSettings: vi.fn(),
	createPreview: vi.fn(),
	deletePreview: vi.fn(),
	updatePreviewConfig: vi.fn(),
	listOpenPRs: vi.fn(),
	getMetrics: vi.fn(),
	listRegistryCredentials: vi.fn(),
	createRegistryCredential: vi.fn(),
	deleteRegistryCredential: vi.fn(),
	setAppRegistryCredential: vi.fn(),
	listGitHubApps: vi.fn(),
	listGitHubRepos: vi.fn(),
	listGitHubBranches: vi.fn(),
	setAppGitHubApp: vi.fn(),
	getApp: vi.fn()
}));

vi.mock('$lib/auth.svelte', () => ({ isAdmin: () => true, can: () => true }));

vi.mock('$app/state', () => ({
	page: state.page
}));

vi.mock('$app/navigation', () => ({
	goto
}));

vi.mock('$lib/components/dialogs/confirm-delete-dialog.svelte', async () => ({
	default: (await import('../mocks/ConfirmDeleteDialog.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-overview.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-logs.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-deployments.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-environment.svelte', async () => ({
	default: (await import('../mocks/AppEnvironmentHarness.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-domain.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-health.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-volumes.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-previews.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/metrics/time-chart.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-history.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	appContext,
	openFeedPanel
}));

vi.mock('$lib/api', () => ({
	...api
}));

import AppDetailPage from '../../routes/apps/[id]/+page.svelte';

const app: App = {
	id: 'app_1',
	name: 'api-service',
	domain: 'api.example.com',
	source_type: 'git',
	source_url: 'https://github.com/example/api-service',
	status: 'running',
	container_port: 8080,
	host_port: 32001,
	memory_limit: null,
	cpu_limit: null,
	health_check_path: '/healthz',
	health_check_interval: 30,
	health_failure_threshold: 3,
	health_consecutive_failures: 0,
	volumes: [],
	drain_timeout: 30,
	build_method: 'tbuild',
	auto_deploy: false,
	watch_branch: 'main',
	deploy_key_pub: null,
	project_id: 'proj_1',
	registry_credential_id: null,
	github_app_id: null,
	dockerfile_content: null,
	compose_content: null,
	base_dir: null,
	static_build: false,
	preview_enabled: false,
	preview_shared_keys: [],
	created_at: '2026-01-01T00:00:00.000Z',
	updated_at: '2026-01-01T00:00:00.000Z'
};

function renderPage(appOverride = app) {
	render(AppDetailPage, { props: { data: { app: appOverride }, params: { id: 'app_1' } } });
}

describe('apps/[id] page', () => {
	beforeEach(() => {
		state.page.url = new URL('http://localhost/apps/app_1');
		state.page.params = { id: 'app_1' };
		goto.mockReset();
		setUiContext.mockReset();
		clearUiContext.mockReset();
		appContext.mockClear();
		openFeedPanel.mockReset();
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.listDeployments.mockResolvedValue([]);
		api.listPreviews.mockResolvedValue([]);
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: 'example-previews.com'
		});
		api.listOpenPRs.mockResolvedValue([]);
		api.getMetrics.mockResolvedValue({
			range: '1h',
			step: 30,
			from: 0,
			to: 3600,
			mem_limit: null,
			series: { t: [], cpu: [], cpu_max: [], mem: [], mem_max: [], rx: [], tx: [], pids: [] },
			events: [],
			current: null
		});
		api.listRegistryCredentials.mockResolvedValue([]);
		api.listGitHubApps.mockResolvedValue([]);
		api.listGitHubRepos.mockResolvedValue([]);
		api.listGitHubBranches.mockResolvedValue([]);
		api.getApp.mockResolvedValue({ ...app });
	});

	it('renders app detail from route data without an initial loading state', () => {
		renderPage();

		expect(screen.queryByText(/^loading\.\.\.$/i)).not.toBeInTheDocument();
		expect(screen.getByRole('heading', { name: /api-service/i })).toBeInTheDocument();
		expect(screen.getByText(/source: git/i)).toBeInTheDocument();
		expect(setUiContext).toHaveBeenCalledWith(
			expect.objectContaining({ type: 'app', app: expect.objectContaining({ id: 'app_1' }) })
		);
	});

	it('fills the settings forms with the saved values on first load', async () => {
		const user = userEvent.setup();
		renderPage({ ...app, source_url: 'https://github.com/o/r' });

		await user.click(screen.getByRole('tab', { name: 'Git' }));
		expect(screen.getByLabelText('Git URL')).toHaveValue('https://github.com/o/r');

		expect(api.getApp).not.toHaveBeenCalled();
	});

	it('re-deploys the app and refreshes its data', async () => {
		api.deployApp.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({ ...app, status: 'building' });

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /re-deploy/i }));

		expect(openFeedPanel).toHaveBeenCalledOnce();
		expect(api.deployApp).toHaveBeenCalledWith('app_1');
		expect(api.getApp).toHaveBeenCalledWith('app_1');
	});

	it('stops a running app and refreshes its data', async () => {
		api.stopApp.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({ ...app, status: 'stopped' });

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^stop$/i }));

		expect(openFeedPanel).toHaveBeenCalledOnce();
		expect(api.stopApp).toHaveBeenCalledWith('app_1');
		expect(api.getApp).toHaveBeenCalledWith('app_1');
	});

	it('starts a stopped app and refreshes its data', async () => {
		api.startApp.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({ ...app, status: 'running' });

		const user = userEvent.setup();
		renderPage({ ...app, status: 'stopped' });

		await user.click(screen.getByRole('button', { name: /^start$/i }));

		expect(openFeedPanel).toHaveBeenCalledOnce();
		expect(api.startApp).toHaveBeenCalledWith('app_1');
		expect(api.getApp).toHaveBeenCalledWith('app_1');
	});

	it('deletes the app after confirmation and redirects home', async () => {
		api.deleteApp.mockResolvedValue(undefined);

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^delete$/i }));
		await user.click(screen.getByRole('button', { name: /confirm delete/i }));

		expect(api.deleteApp).toHaveBeenCalledWith('app_1');
		expect(goto).toHaveBeenCalledWith('/');
	});

	it('shows an inline error when redeploy fails', async () => {
		api.deployApp.mockRejectedValue(new Error('Deploy failed'));

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /re-deploy/i }));

		expect(await screen.findByText(/deploy failed/i)).toBeInTheDocument();
		expect(api.getApp).not.toHaveBeenCalled();
	});

	it('wires the environment tab to the registry credential and redeploy actions', async () => {
		api.setAppRegistryCredential.mockResolvedValue(undefined);
		api.deployApp.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({
			...app,
			source_type: 'image',
			registry_credential_id: 'cred_1'
		});

		const user = userEvent.setup();
		renderPage({ ...app, source_type: 'image', registry_credential_id: null });

		await user.click(screen.getByRole('tab', { name: /environment/i }));
		expect(screen.getByText('Environment for app_1')).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /attach credential/i }));
		expect(api.setAppRegistryCredential).toHaveBeenCalledWith('app_1', 'cred_1');

		await user.click(screen.getByRole('button', { name: /redeploy from environment/i }));
		expect(api.deployApp).toHaveBeenCalledWith('app_1');
		expect(api.getApp).toHaveBeenCalledWith('app_1');
	});

	it('saves resource limits and updates app context', async () => {
		api.updateAppResources.mockResolvedValue({
			...app,
			memory_limit: 2 * 1024 * 1024 * 1024,
			cpu_limit: 1_500_000_000
		});

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('tab', { name: /resources/i }));
		const numberInputs = document.querySelectorAll('input[type="number"]');
		await user.clear(numberInputs[0] as HTMLInputElement);
		await user.type(numberInputs[0] as HTMLInputElement, '2');
		await user.clear(numberInputs[1] as HTMLInputElement);
		await user.type(numberInputs[1] as HTMLInputElement, '1.5');
		await user.click(screen.getByRole('button', { name: /^save$/i }));

		expect(api.updateAppResources).toHaveBeenCalledWith('app_1', 2 * 1024 * 1024, 1_500_000_000);
		expect(setUiContext).toHaveBeenCalledWith(
			expect.objectContaining({
				type: 'app',
				app: expect.objectContaining({ cpu_limit: 1_500_000_000 })
			})
		);
		expect(await screen.findByText(/saved/i)).toBeInTheDocument();
	});

	function webhookConfig(secret: string) {
		const paths = {
			github: '/api/webhooks/github/app_1',
			gitlab: '/api/webhooks/gitlab/app_1',
			gitea: '/api/webhooks/gitea/app_1',
			generic: '/api/webhooks/generic/app_1'
		};
		return {
			app_id: 'app_1',
			paths,
			urls: {
				github: 'https://dash.example.com/api/webhooks/github/app_1',
				gitlab: 'https://dash.example.com/api/webhooks/gitlab/app_1',
				gitea: 'https://dash.example.com/api/webhooks/gitea/app_1',
				generic: 'https://dash.example.com/api/webhooks/generic/app_1'
			},
			has_secret: true,
			secret,
			auto_deploy: false,
			watch_branch: 'main'
		};
	}

	it('shows the webhook URL, and the secret only after an explicit reveal', async () => {
		api.getWebhookConfig.mockResolvedValue(webhookConfig('generated-secret-0123456789'));
		api.revealWebhookSecret.mockResolvedValue({ secret: 'generated-secret-0123456789' });

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('tab', { name: /^git$/i }));

		const url = (await screen.findByLabelText(/payload url/i)) as HTMLInputElement;
		expect(url.value).toBe('https://dash.example.com/api/webhooks/github/app_1');
		const secretField = screen.getByLabelText(/^secret$/i) as HTMLInputElement;
		expect(secretField.value).not.toContain('generated-secret');
		expect(api.revealWebhookSecret).not.toHaveBeenCalled();
		await user.click(screen.getByRole('button', { name: /^reveal$/i }));
		expect(api.revealWebhookSecret).toHaveBeenCalledWith('app_1');
		expect(await screen.findByDisplayValue('generated-secret-0123456789')).toBeInTheDocument();
		expect(screen.getByText(/just the push event/i)).toBeInTheDocument();

		await user.click(screen.getByRole('tab', { name: /^gitlab$/i }));
		expect((screen.getByLabelText(/^url$/i) as HTMLInputElement).value).toBe(
			'https://dash.example.com/api/webhooks/gitlab/app_1'
		);
		expect(screen.getByLabelText(/secret token/i)).toBeInTheDocument();

		await user.click(screen.getByRole('tab', { name: /gitea/i }));
		expect((screen.getByLabelText(/target url/i) as HTMLInputElement).value).toBe(
			'https://dash.example.com/api/webhooks/gitea/app_1'
		);
	});

	it('rotates the webhook secret after confirmation', async () => {
		api.getWebhookConfig.mockResolvedValue(webhookConfig('old-secret-0123456789'));
		api.rotateWebhookSecret.mockResolvedValue(webhookConfig('new-secret-0123456789'));

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('tab', { name: /^git$/i }));
		await user.click(await screen.findByRole('button', { name: /rotate secret/i }));
		expect(api.rotateWebhookSecret).not.toHaveBeenCalled();
		await user.click(screen.getByRole('button', { name: /rotate now/i }));

		expect(api.rotateWebhookSecret).toHaveBeenCalledWith('app_1');
		expect(await screen.findByText(/new secret generated/i)).toBeInTheDocument();
		expect((screen.getByLabelText(/^secret$/i) as HTMLInputElement).value).toBe(
			'new-secret-0123456789'
		);
	});

	it('sets a custom webhook secret and rejects short ones locally', async () => {
		api.getWebhookConfig.mockResolvedValue(webhookConfig('old-secret-0123456789'));
		api.setWebhookSecret.mockResolvedValue(webhookConfig('my-own-secret-value-42'));

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('tab', { name: /^git$/i }));
		await user.click(await screen.findByText(/use your own secret/i));

		const input = screen.getByLabelText(/custom secret/i);
		await user.type(input, 'short');
		await user.click(
			within(screen.getByTestId('app-webhooks')).getByRole('button', { name: /^save$/i })
		);
		expect(api.setWebhookSecret).not.toHaveBeenCalled();
		expect(await screen.findByText(/at least 16 characters/i)).toBeInTheDocument();

		await user.clear(input);
		await user.type(input, 'my-own-secret-value-42');
		await user.click(
			within(screen.getByTestId('app-webhooks')).getByRole('button', { name: /^save$/i })
		);

		expect(api.setWebhookSecret).toHaveBeenCalledWith('app_1', 'my-own-secret-value-42');
		expect(await screen.findByText(/webhook secret updated/i)).toBeInTheDocument();
	});

	it('enables preview deployments from the previews tab', async () => {
		api.updatePreviewConfig.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({ ...app, preview_enabled: true });

		const user = userEvent.setup();
		renderPage({ ...app, github_app_id: 'gh_1', preview_enabled: false });

		await user.click(screen.getByRole('tab', { name: /previews/i }));
		await user.click(screen.getByRole('button', { name: /enable pr previews/i }));

		expect(api.updatePreviewConfig).toHaveBeenCalledWith('app_1', true);
		expect(api.getApp).toHaveBeenCalledWith('app_1');
	});

	it('explains and blocks previews when no preview domain is configured', async () => {
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: ''
		});

		const user = userEvent.setup();
		renderPage({ ...app, github_app_id: 'gh_1', preview_enabled: false });

		await user.click(screen.getByRole('tab', { name: /previews/i }));

		expect(await screen.findByText(/previews need a preview domain/i)).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /enable pr previews/i })).toBeDisabled();
		expect(screen.getByRole('button', { name: /create preview/i })).toBeDisabled();
	});

	it('loads open pull requests for previews', async () => {
		api.listOpenPRs.mockResolvedValue([
			{
				number: 42,
				title: 'Add feature',
				head_ref: 'feature-branch',
				head_sha: 'abc123',
				pr_url: 'https://github.com/example/repo/pull/42'
			}
		]);

		const user = userEvent.setup();
		renderPage({ ...app, github_app_id: 'gh_1', preview_enabled: true });

		await user.click(screen.getByRole('tab', { name: /previews/i }));
		await user.click(screen.getByRole('button', { name: /load open prs/i }));

		expect(api.listOpenPRs).toHaveBeenCalledWith('app_1');
		expect(await screen.findByText(/add feature/i)).toBeInTheDocument();
	});

	describe('lifecycle', () => {
		class FakeWebSocket {
			static instances: FakeWebSocket[] = [];
			url: string;
			closed = false;
			onmessage: ((evt: { data: string }) => void) | null = null;
			onerror: (() => void) | null = null;
			onclose: (() => void) | null = null;
			constructor(url: string) {
				this.url = url;
				FakeWebSocket.instances.push(this);
			}
			close() {
				this.closed = true;
			}
		}

		beforeEach(() => {
			FakeWebSocket.instances = [];
			vi.stubGlobal('WebSocket', FakeWebSocket);
		});

		afterEach(() => {
			vi.unstubAllGlobals();
		});

		it('keeps the log stream open and the feed paused when the app reloads', async () => {
			api.deployApp.mockResolvedValue(undefined);
			api.getApp.mockResolvedValue({ ...app, status: 'running' });

			const user = userEvent.setup();
			renderPage();

			await user.click(screen.getByRole('tab', { name: /^logs$/i }));
			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].url).toMatch(/\/api\/apps\/app_1\/logs\/stream$/);

			// Re-deploying reloads the app data, which used to tear everything down.
			await user.click(screen.getByRole('button', { name: /re-deploy/i }));
			await vi.waitFor(() => expect(api.getApp).toHaveBeenCalled());

			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].closed).toBe(false);
			expect(clearUiContext).not.toHaveBeenCalled();
			expect(setUiContext).toHaveBeenLastCalledWith(
				expect.objectContaining({ feed: expect.objectContaining({ wsUrl: null }) })
			);
		});

		it('closes the log stream when leaving the Logs tab', async () => {
			const user = userEvent.setup();
			renderPage();

			await user.click(screen.getByRole('tab', { name: /^logs$/i }));
			await user.click(screen.getByRole('tab', { name: /overview/i }));

			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].closed).toBe(true);
		});

		it('keeps a single metrics poller across app reloads', async () => {
			api.deployApp.mockResolvedValue(undefined);
			api.getApp.mockResolvedValue({ ...app, status: 'running' });

			const user = userEvent.setup();
			renderPage();

			await user.click(screen.getByRole('tab', { name: /metrics/i }));
			await vi.waitFor(() => expect(api.getMetrics).toHaveBeenCalledTimes(1));
			expect(api.getMetrics).toHaveBeenCalledWith('apps', app.id, '1h');

			await user.click(screen.getByRole('button', { name: /re-deploy/i }));
			await vi.waitFor(() => expect(api.getApp).toHaveBeenCalled());

			expect(api.getMetrics).toHaveBeenCalledTimes(1);
		});
	});
});
