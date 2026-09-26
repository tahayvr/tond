import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { App } from '$lib/api';

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
	getApp: vi.fn(),
	updateContainerPort: vi.fn(),
	openLogStream: vi.fn((url: string) => {
		const ws = new WebSocket(url);
		return () => ws.close();
	})
}));

const authz = vi.hoisted(() => ({ deny: new Set<string>() }));
vi.mock('$lib/auth.svelte', () => ({
	isAdmin: () => true,
	can: (scope: string) => !authz.deny.has(scope)
}));

vi.mock('$app/state', async () => ({
	page: (await import('../mocks/page-state.svelte')).page
}));

vi.mock('$app/navigation', () => ({
	goto,
	invalidateAll: vi.fn()
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

vi.mock('$lib/components/tabs/app-page/app-readiness.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/tabs/app-page/app-port.svelte', async () => ({
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
	...api,
	appReadiness: (a: { readiness?: string }) => a.readiness ?? 'http'
}));

import AppDetailPage from '../../routes/apps/[id]/+page.svelte';
import { setPageUrl } from '../mocks/page-state.svelte';
import { confirmDialog, renderConfirmHost } from '../confirm';
import { tick } from 'svelte';

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
	renderConfirmHost();
	render(AppDetailPage, { props: { data: { app: appOverride }, params: { id: 'app_1' } } });
}

/** Tabs live in the URL: "navigate" by changing the page URL. */
async function goToTab(query: string) {
	setPageUrl(`http://localhost/apps/app_1${query}`, { id: 'app_1' });
	await tick();
}

describe('apps/[id] page', () => {
	beforeEach(() => {
		setPageUrl('http://localhost/apps/app_1', { id: 'app_1' });
		authz.deny.clear();
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
		api.openLogStream.mockImplementation((url: string) => {
			const ws = new WebSocket(url);
			return () => ws.close();
		});
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
		renderPage({ ...app, source_url: 'https://github.com/o/r' });

		await goToTab('?tab=settings&section=git');
		expect(screen.getByLabelText('Git URL')).toHaveValue('https://github.com/o/r');

		expect(api.getApp).not.toHaveBeenCalled();
	});

	it('re-deploys the app and opens the live deploy view', async () => {
		api.deployApp.mockResolvedValue({
			message: 'Deployment started',
			app_id: 'app_1',
			deployment_id: 'dep_9'
		});

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^deploy$/i }));

		expect(api.deployApp).toHaveBeenCalledWith('app_1');
		expect(goto).toHaveBeenCalledWith('/apps/app_1/deployments/dep_9');
	});

	it('links to the running deployment while the app builds', async () => {
		api.listDeployments.mockResolvedValue([
			{ id: 'dep_live', status: 'running' },
			{ id: 'dep_old', status: 'succeeded' }
		]);
		renderPage({ ...app, status: 'building' });

		const link = await screen.findByRole('link', { name: /view deploy/i });
		expect(link).toHaveAttribute('href', '/apps/app_1/deployments/dep_live');
	});

	it('opens the tab named in ?tab=', async () => {
		setPageUrl('http://localhost/apps/app_1?tab=deployments', { id: 'app_1' });
		renderPage();
		expect(
			within(screen.getByRole('navigation', { name: 'App sections' })).getByRole('link', {
				name: 'Deployments'
			})
		).toHaveAttribute('aria-current', 'page');
		await vi.waitFor(() => expect(api.listDeployments).toHaveBeenCalledWith('app_1'));
	});

	it('stops a running app and refreshes its data', async () => {
		api.stopApp.mockResolvedValue(undefined);
		api.getApp.mockResolvedValue({ ...app, status: 'stopped' });

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^stop$/i }));
		expect(api.stopApp).not.toHaveBeenCalled();
		await confirmDialog(user);

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

	it('keeps delete out of the header and requires typing the name in the danger zone', async () => {
		api.deleteApp.mockResolvedValue(undefined);

		const user = userEvent.setup();
		renderPage();

		expect(screen.queryByRole('button', { name: /delete/i })).not.toBeInTheDocument();
		await goToTab('?tab=settings&section=danger');
		await user.click(screen.getByRole('button', { name: /delete app/i }));

		const dialog = await screen.findByRole('alertdialog');
		const submit = within(dialog).getByRole('button', { name: /delete app/i });
		expect(submit).toBeDisabled();
		await confirmDialog(user, { typed: 'api-service' });

		expect(api.deleteApp).toHaveBeenCalledWith('app_1');
		expect(goto).toHaveBeenCalledWith('/apps', { invalidateAll: true });
	});

	it('restarts only after confirmation', async () => {
		api.restartApp.mockResolvedValue(undefined);
		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^restart$/i }));
		expect(api.restartApp).not.toHaveBeenCalled();
		await confirmDialog(user);
		expect(api.restartApp).toHaveBeenCalledWith('app_1');
	});

	it('ignores a double click on deploy while the first request is pending', async () => {
		let resolve: () => void = () => {};
		api.deployApp.mockReturnValue(
			new Promise((r) => (resolve = () => r({ deployment_id: 'dep_9' })))
		);
		const user = userEvent.setup();
		renderPage();

		const button = screen.getByRole('button', { name: /^deploy$/i });
		await user.click(button);
		await user.click(button);
		expect(api.deployApp).toHaveBeenCalledTimes(1);
		expect(screen.getByRole('button', { name: /starting deploy/i })).toBeDisabled();
		resolve();
	});

	it('hides what a viewer can’t use and explains read-only settings', async () => {
		for (const scope of ['apps:deploy', 'apps:control', 'apps:write', 'apps:delete', 'logs:read']) {
			authz.deny.add(scope);
		}
		setPageUrl('http://localhost/apps/app_1?tab=settings&section=git', { id: 'app_1' });
		renderPage();

		expect(screen.queryByRole('button', { name: /^deploy$/i })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /^stop$/i })).not.toBeInTheDocument();
		const tabs = screen.getByRole('navigation', { name: 'App sections' });
		expect(within(tabs).queryByRole('link', { name: 'Logs' })).not.toBeInTheDocument();
		const sections = screen.getByRole('navigation', { name: 'Settings sections' });
		expect(within(sections).queryByRole('link', { name: 'Danger zone' })).not.toBeInTheDocument();
		expect(screen.getByText(/changing them needs the/i)).toBeInTheDocument();
		// The webhook config (apps:write) is explained instead of failing with a 403.
		expect(api.getWebhookConfig).not.toHaveBeenCalled();
		expect(screen.getByText(/webhook urls and secrets are shown to users/i)).toBeInTheDocument();

		await goToTab('?tab=logs');
		expect(
			screen.getByText(/your role can’t open this section|your role can't open this section/i)
		).toBeInTheDocument();
	});

	it('opens the tab named in the URL and groups settings sections', async () => {
		setPageUrl('http://localhost/apps/app_1?tab=domain', { id: 'app_1' });
		renderPage();

		// Old flat links (?tab=domain) land on Settings → Domain.
		const sections = screen.getByRole('navigation', { name: 'Settings sections' });
		expect(within(sections).getByRole('link', { name: 'Domain' })).toHaveAttribute(
			'aria-current',
			'page'
		);
		const tabs = screen.getByRole('navigation', { name: 'App sections' });
		expect(
			within(tabs)
				.getAllByRole('link')
				.map((a) => a.textContent?.trim())
		).toEqual(['Overview', 'Deployments', 'Logs', 'Metrics', 'Uptime', 'Settings']);
		expect(within(tabs).getByRole('link', { name: 'Logs' })).toHaveAttribute(
			'href',
			'/apps/app_1?tab=logs'
		);
	});

	it('shows an inline error when redeploy fails', async () => {
		api.deployApp.mockRejectedValue(new Error('Deploy failed'));

		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /^deploy$/i }));

		expect(await screen.findByText(/deploy failed/i)).toBeInTheDocument();
		expect(api.getApp).not.toHaveBeenCalled();
	});

	it('wires the environment tab to the registry credential and redeploy actions', async () => {
		api.setAppRegistryCredential.mockResolvedValue(undefined);
		api.deployApp.mockResolvedValue({
			message: 'Deployment started',
			app_id: 'app_1',
			deployment_id: 'dep_env'
		});
		api.getApp.mockResolvedValue({
			...app,
			source_type: 'image',
			registry_credential_id: 'cred_1'
		});

		const user = userEvent.setup();
		renderPage({ ...app, source_type: 'image', registry_credential_id: null });

		await goToTab('?tab=settings&section=environment');
		expect(screen.getByText('Environment for app_1')).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /attach credential/i }));
		expect(api.setAppRegistryCredential).toHaveBeenCalledWith('app_1', 'cred_1');

		await user.click(screen.getByRole('button', { name: /redeploy from environment/i }));
		expect(api.deployApp).toHaveBeenCalledWith('app_1');
		expect(goto).toHaveBeenCalledWith('/apps/app_1/deployments/dep_env');
	});

	it('saves resource limits and updates app context', async () => {
		api.updateAppResources.mockResolvedValue({
			...app,
			memory_limit: 2 * 1024 * 1024 * 1024,
			cpu_limit: 1_500_000_000
		});

		const user = userEvent.setup();
		renderPage();

		await goToTab('?tab=settings&section=resources');
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

		await goToTab('?tab=settings&section=git');

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

		await goToTab('?tab=settings&section=git');
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

		await goToTab('?tab=settings&section=git');
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

		await goToTab('?tab=settings&section=previews');
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

		renderPage({ ...app, github_app_id: 'gh_1', preview_enabled: false });

		await goToTab('?tab=settings&section=previews');

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

		await goToTab('?tab=settings&section=previews');
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
			api.restartApp.mockResolvedValue(undefined);
			api.getApp.mockResolvedValue({ ...app, status: 'running' });

			const user = userEvent.setup();
			renderPage();

			await goToTab('?tab=logs');
			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].url).toMatch(/\/api\/apps\/app_1\/logs\/stream$/);

			// Restarting reloads the app data, which used to tear everything down.
			await user.click(screen.getByRole('button', { name: /^restart$/i }));
			await confirmDialog(user);
			await vi.waitFor(() => expect(api.getApp).toHaveBeenCalled());

			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].closed).toBe(false);
			expect(clearUiContext).not.toHaveBeenCalled();
			expect(setUiContext).toHaveBeenLastCalledWith(
				expect.objectContaining({ feed: expect.objectContaining({ wsUrl: null }) })
			);
		});

		it('closes the log stream when leaving the Logs tab', async () => {
			renderPage();

			await goToTab('?tab=logs');
			await goToTab('');

			expect(FakeWebSocket.instances).toHaveLength(1);
			expect(FakeWebSocket.instances[0].closed).toBe(true);
		});

		it('keeps a single metrics poller across app reloads', async () => {
			api.restartApp.mockResolvedValue(undefined);
			api.getApp.mockResolvedValue({ ...app, status: 'running' });

			const user = userEvent.setup();
			renderPage();

			await goToTab('?tab=metrics');
			await vi.waitFor(() => expect(api.getMetrics).toHaveBeenCalledTimes(1));
			expect(api.getMetrics).toHaveBeenCalledWith('apps', app.id, '1h');

			await user.click(screen.getByRole('button', { name: /^restart$/i }));
			await confirmDialog(user);
			await vi.waitFor(() => expect(api.getApp).toHaveBeenCalled());

			expect(api.getMetrics).toHaveBeenCalledTimes(1);
		});
	});
});
