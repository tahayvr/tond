import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, projectsContext, getUiContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' })),
	getUiContext: vi.fn(() => ({ type: 'idle', feed: { title: 'Feed', wsUrl: null } }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext,
	getUiContext
}));

import AppsPage from '../../routes/apps/+page.svelte';

describe('apps page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders apps from route data without loading state', () => {
		render(AppsPage, {
			props: {
				params: {},
				data: {
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
						}
					],
					projects: [
						{
							id: 'proj_1',
							name: 'Platform',
							description: 'Core project',
							env_vars: {},
							active_preview_count: 0,
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					]
				}
			}
		});

		expect(screen.queryByText(/loading_units/i)).not.toBeInTheDocument();
		expect(screen.getByText(/1 of 1 running/i)).toBeInTheDocument();
	});
});
