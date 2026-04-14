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

import SystemPage from '../../routes/system/+page.svelte';

describe('system page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders system metrics from route data without loading state', () => {
		render(SystemPage, {
			props: {
				params: {},
				data: {
					metrics: {
						cpu_percent: 20,
						mem_used: 512,
						mem_total: 2048,
						mem_percent: 25,
						disk_used: 10 * 1024 * 1024 * 1024,
						disk_total: 100 * 1024 * 1024 * 1024,
						disk_percent: 10,
						containers_running: 3,
						apps_total: 5,
						apps_running: 4,
						collected_at: '2026-01-01T00:00:00.000Z'
					}
				}
			}
		});

		expect(screen.queryByText(/loading_node_status/i)).not.toBeInTheDocument();
		expect(screen.getByText(/system/i)).toBeInTheDocument();
	});
});
