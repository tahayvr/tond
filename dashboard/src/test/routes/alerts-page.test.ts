import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { setUiContext, clearUiContext, alertsContext, getUiContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	alertsContext: vi.fn(() => ({ type: 'alerts' })),
	getUiContext: vi.fn(() => ({ type: 'idle', feed: { title: 'Feed', wsUrl: null } }))
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	alertsContext,
	getUiContext
}));

import AlertsPage from '../../routes/alerts/+page.svelte';

describe('alerts page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		alertsContext.mockClear();
	});

	it('renders alert rules from route data without loading state', () => {
		render(AlertsPage, {
			props: {
				params: {},
				data: {
					rules: [
						{
							id: 'alert_1',
							app_id: null,
							name: 'CPU high',
							enabled: true,
							metric: 'cpu',
							threshold: 90,
							has_webhook_url: true,
							webhook_host: 'hooks.example.com',
							events: [],
							last_fired_at: null,
							created_at: '2026-01-01T00:00:00.000Z',
							updated_at: '2026-01-01T00:00:00.000Z'
						}
					]
				}
			}
		});

		expect(screen.queryByText(/loading_monitors/i)).not.toBeInTheDocument();
		expect(screen.getByText(/1 of 1 rules enabled/i)).toBeInTheDocument();
	});
});
