import { render, screen, waitFor, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { goto, setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	goto: vi.fn(),
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$app/navigation', () => ({
	goto
}));

const api = vi.hoisted(() => ({
	getOnboardingStatus: vi.fn(),
	getSystemMetrics: vi.fn()
}));

vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	...api
}));

const FRESH_STATUS = {
	dashboard_domain: '',
	is_fresh_install: true,
	has_projects: false,
	has_apps: false,
	has_databases: false,
	has_cron_jobs: false,
	has_source_integration: true,
	has_successful_deploy: false,
	has_notification_channel: false,
	last_deployment: null,
	server_public_ip: '203.0.113.10',
	domain_status: {
		configured: false,
		expected_target: '203.0.113.10',
		dns_matches_server: null,
		https_ready: null,
		certificate_status: null
	}
};

vi.mock('$lib/auth.svelte', () => ({
	can: () => true,
	getUser: () => ({ email: 'admin@example.com', role: 'admin', totp_enabled: false })
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext
}));

import DashboardHome from '../../routes/+page.svelte';

const data = {
	projects: [
		{
			id: 'proj_1',
			name: 'web-platform',
			description: 'Main product',
			env: [],
			active_preview_count: 1,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z'
		}
	],
	apps: [
		{
			id: 'app_running',
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
			preview_excluded_keys: [],
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z'
		},
		{
			id: 'app_unassigned',
			name: 'worker',
			domain: null,
			source_type: 'git',
			source_url: 'https://github.com/example/worker',
			status: 'failed',
			container_port: 4000,
			host_port: null,
			memory_limit: null,
			cpu_limit: null,
			health_check_path: null,
			health_check_interval: 30,
			health_failure_threshold: 3,
			health_consecutive_failures: 0,
			volumes: [],
			drain_timeout: 30,
			build_method: 'tbuild',
			auto_deploy: false,
			watch_branch: null,
			deploy_key_pub: null,
			project_id: null,
			registry_credential_id: null,
			github_app_id: null,
			dockerfile_content: null,
			compose_content: null,
			base_dir: null,
			static_build: false,
			preview_enabled: false,
			preview_excluded_keys: [],
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z'
		}
	],
	databases: [
		{
			id: 'db_1',
			name: 'app-postgres',
			db_type: 'postgres',
			project_id: 'proj_1',
			host_port: 5401,
			status: 'running',
			db_name: 'app_db',
			db_user: 'app_user',
			has_password: true,
			has_root_password: false,
			memory_limit: null,
			custom_external_url: null,
			custom_internal_url: null,
			image_used: 'postgres:16-alpine',
			custom_image: null,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z',
			public_access: false
		}
	],
	cronJobs: [
		{
			id: 'cron_1',
			name: 'hourly-report',
			schedule: '0 * * * *',
			image: 'curlimages/curl:latest',
			command: null,
			project_id: 'proj_1',
			enabled: true,
			last_run_at: null,
			next_run_at: null,
			status: 'idle',
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z',
			timeout_seconds: null,
			env: [],
			last_run_status: null,
			concurrency_policy: 'skip',
			memory_mb: null,
			cpu_shares: null,
			timezone: null,
			max_retries: 0,
			retry_delay_seconds: 60,
			volumes: null
		}
	],
	recentActivity: [
		{
			seq: 7,
			created_at: '2026-01-01T12:00:00.000Z',
			actor_kind: 'user',
			actor_id: 'usr_1',
			actor_label: 'admin@example.com',
			action: 'deploy',
			resource_type: 'app',
			resource_id: 'app_running',
			resource_name: 'api-service',
			outcome: 'success',
			ip: null,
			detail: 'Deployed sha-123'
		}
	],
	serverName: 'Tond Server'
};

describe('dashboard home page', () => {
	beforeEach(() => {
		goto.mockReset();
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
		api.getOnboardingStatus.mockReset().mockResolvedValue(FRESH_STATUS);
		api.getSystemMetrics.mockReset().mockResolvedValue({
			cpu_percent: 12,
			mem_percent: 40,
			disk_percent: 93,
			mem_used: 0,
			mem_total: 0,
			disk_used: 0,
			disk_total: 0,
			containers_running: 2,
			apps_total: 2,
			apps_running: 1,
			collected_at: '2026-01-01T00:00:00Z'
		});
	});

	it('renders health tiles and the app list from route data without a loading state', async () => {
		render(DashboardHome, { props: { data, params: {} } });

		expect(setUiContext).toHaveBeenCalledWith({ type: 'projects' });
		expect(screen.getByText('Tond Server')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'api-service' })).toHaveAttribute(
			'href',
			'/apps/app_running'
		);
		// The failing app is listed first and flagged.
		expect(screen.getByText(/1 needs attention/i)).toBeInTheDocument();
		expect(screen.getAllByRole('link', { name: /worker|api-service/ })[0]).toHaveTextContent(
			'worker'
		);
		expect(screen.getByText(/1 cron job/i)).toBeInTheDocument();
	});

	it('shows the setup checklist from the onboarding endpoint', async () => {
		render(DashboardHome, { props: { data, params: {} } });

		const checklist = await screen.findByTestId('setup-checklist');
		expect(checklist).toHaveTextContent('1 of 5 done');
		expect(within(checklist).getByRole('link', { name: 'Set domain' })).toHaveAttribute(
			'href',
			'/settings'
		);
		expect(within(checklist).getByRole('link', { name: 'Enable 2FA' })).toHaveAttribute(
			'href',
			'/profile?tab=2fa'
		);
		expect(within(checklist).queryByRole('link', { name: 'Connect' })).not.toBeInTheDocument();
		expect(screen.getByTestId('last-deploy-tile')).toHaveTextContent('Nothing deployed yet.');
	});

	it('links the primary actions instead of navigating from click handlers', () => {
		render(DashboardHome, { props: { data, params: {} } });

		expect(screen.getByRole('link', { name: /new project/i })).toHaveAttribute('href', '/projects');
		expect(screen.getByRole('link', { name: /new app/i })).toHaveAttribute('href', '/apps/new');
	});

	it('clears the ui context on unmount', async () => {
		const view = render(DashboardHome, { props: { data, params: {} } });

		view.unmount();

		await waitFor(() => {
			expect(clearUiContext).toHaveBeenCalled();
		});
	});
});
