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

import CronPage from '../../routes/cron/+page.svelte';

describe('cron page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();
	});

	it('renders cron jobs from route data without loading state', () => {
		render(CronPage, {
			props: {
				params: {},
				data: {
					jobs: [
						{
							id: 'cron_1',
							name: 'nightly',
							schedule: '0 0 * * *',
							image: 'alpine:latest',
							command: null,
							project_id: null,
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
					]
				}
			}
		});

		expect(screen.queryByText(/loading_schedules/i)).not.toBeInTheDocument();
		expect(screen.getByText(/1 of 1 enabled/i)).toBeInTheDocument();
	});
});
