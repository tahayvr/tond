import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, projectsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext
}));

import ActivityPage from '../../routes/activity/+page.svelte';

describe('activity page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders activity from route data without initial loading state', () => {
		render(ActivityPage, {
			props: {
				params: {},
				data: {
					entries: [
						{
							id: 'log_1',
							app_id: 'app_1',
							app_name: 'api-service',
							action: 'deploy',
							actor: 'admin@example.com',
							detail: 'Deployed',
							created_at: '2026-01-01T00:00:00.000Z'
						}
					],
					apps: [
						{
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
							project_id: null,
							registry_credential_id: null,
							github_app_id: null,
							dockerfile_content: null,
							compose_content: null,
							base_dir: null,
							static_build: false,
							preview_enabled: false,
							preview_excluded_keys: [],
							preview_env_vars: {},
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					],
					initialFilterAppId: '',
					initialFilterAction: '',
					initialFilterLimit: 50
				}
			}
		});

		expect(screen.queryByText(/loading_events/i)).not.toBeInTheDocument();
		expect(screen.getByText(/1 events_logged/i)).toBeInTheDocument();
	});
});
