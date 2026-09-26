import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
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
			env_vars: {},
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
			preview_shared_keys: [],
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
			preview_shared_keys: [],
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
			db_password: 'secret',
			root_password: null,
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
			env: null,
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
			id: 'log_1',
			app_id: 'app_running',
			app_name: 'api-service',
			action: 'deploy',
			actor: 'admin@example.com',
			detail: 'Deployed sha-123',
			created_at: '2026-01-01T12:00:00.000Z'
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
	});

	it('renders grouped dashboard data from route data without a local loading state', async () => {
		render(DashboardHome, { props: { data, params: {} } });

		expect(setUiContext).toHaveBeenCalledWith({ type: 'projects' });
		expect(screen.queryByText(/loading server map/i)).not.toBeInTheDocument();
		expect(screen.getByText('Tond Server')).toBeInTheDocument();
		expect(screen.getAllByText('api-service').length).toBeGreaterThan(0);
		expect(screen.getByText('app-postgres')).toBeInTheDocument();
		expect(screen.getByText('hourly-report')).toBeInTheDocument();
		expect(screen.getByText('UNASSIGNED_RESOURCES')).toBeInTheDocument();
		expect(screen.getByText('worker')).toBeInTheDocument();
		expect(screen.getByText('1/2')).toBeInTheDocument();
	});

	it('navigates to the projects page from the new project action', async () => {
		const user = userEvent.setup();
		render(DashboardHome, { props: { data, params: {} } });

		await user.click(screen.getByRole('button', { name: /new project/i }));

		expect(goto).toHaveBeenCalledWith('/projects');
	});

	it('clears the ui context on unmount', async () => {
		const view = render(DashboardHome, { props: { data, params: {} } });

		view.unmount();

		await waitFor(() => {
			expect(clearUiContext).toHaveBeenCalled();
		});
	});
});
