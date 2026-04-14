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

import ProjectsPage from '../../routes/projects/+page.svelte';

describe('projects page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders projects from route data without loading state', () => {
		render(ProjectsPage, {
			props: {
				params: {},
				data: {
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
							project_id: 'proj_1',
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
					]
				}
			}
		});

		expect(screen.queryByText(/^loading\.\.\.$/i)).not.toBeInTheDocument();
		expect(screen.getByText('Platform')).toBeInTheDocument();
		expect(setUiContext).toHaveBeenCalledWith({ type: 'projects' });
	});
});
