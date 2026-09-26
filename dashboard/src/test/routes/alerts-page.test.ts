import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ActiveAlert, AlertRule } from '$lib/api';
import { MOCK_ACTIVE_ALERTS, MOCK_ALERT_RULES } from '../../mocks/fixtures';

const api = vi.hoisted(() => ({
	listActiveAlerts: vi.fn(),
	listAlertHistory: vi.fn(),
	acknowledgeAlert: vi.fn(),
	snoozeAlert: vi.fn(),
	unsnoozeAlert: vi.fn(),
	getAlertSettings: vi.fn(),
	updateAlertSettings: vi.fn(),
	getAlertEpisode: vi.fn()
}));

vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	...api
}));
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));
vi.mock('$app/state', async () => ({
	page: (await import('../mocks/page-state.svelte')).page
}));
vi.mock('$app/navigation', () => ({ goto: vi.fn(), invalidateAll: vi.fn() }));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	alertsContext: vi.fn(() => ({ type: 'alerts' }))
}));

import { setPageUrl } from '../mocks/page-state.svelte';
import AlertsPage from '../../routes/alerts/+page.svelte';

const rules = structuredClone(MOCK_ALERT_RULES) as AlertRule[];
const active = structuredClone(MOCK_ACTIVE_ALERTS) as ActiveAlert[];

function renderPage(activeAlerts: ActiveAlert[] = active) {
	return render(AlertsPage, {
		props: { params: {}, data: { rules, active: activeAlerts, apps: [] } }
	});
}

describe('alerts page', () => {
	beforeEach(() => {
		setPageUrl('http://localhost/alerts');
		for (const fn of Object.values(api)) fn.mockReset();
		api.listActiveAlerts.mockResolvedValue(active);
		api.listAlertHistory.mockResolvedValue({ episodes: [], total: 0 });
		api.acknowledgeAlert.mockResolvedValue(undefined);
		api.snoozeAlert.mockResolvedValue(undefined);
	});

	it('shows firing alerts first, then pending ones, with delivery problems called out', async () => {
		renderPage();
		const firing = screen.getByRole('heading', { name: /firing now/i });
		expect(firing).toHaveTextContent('(2)');
		expect(screen.getByRole('heading', { name: /pending/i })).toBeInTheDocument();
		expect(screen.getByText(/94.6% \(alert above 90.0%\)/)).toBeInTheDocument();
		expect(screen.getByText(/acknowledged by admin@example.com/i)).toBeInTheDocument();
		expect(screen.getByText(/1 notification couldn't be delivered/i)).toBeInTheDocument();
		expect(screen.getByText(/some notifications couldn't be delivered/i)).toBeInTheDocument();
		expect(await screen.findByText(/no alerts have fired yet/i)).toBeInTheDocument();
	});

	it('acknowledges and snoozes an alert', async () => {
		const user = userEvent.setup();
		renderPage();
		const list = screen.getAllByRole('list', { name: /active alerts/i })[0];
		const first = within(list).getAllByRole('listitem')[0];

		await user.click(within(first).getByRole('button', { name: /acknowledge/i }));
		expect(api.acknowledgeAlert).toHaveBeenCalledWith('ep_1');
		expect(api.listActiveAlerts).toHaveBeenCalled();

		await user.click(within(first).getByRole('button', { name: /snooze/i }));
		// The popover is portalled and positioned by floating-ui (not laid out in jsdom).
		await user.click(await screen.findByText('Until resolved'));
		expect(api.snoozeAlert).toHaveBeenCalledWith('alert_1', {
			target: 'app:app_running',
			minutes: undefined,
			until_resolved: true
		});
	});

	it('says all clear when nothing is firing', () => {
		renderPage([]);
		expect(screen.getByText(/all clear/i)).toBeInTheDocument();
		expect(screen.getByText(/5 rules are watching/i)).toBeInTheDocument();
	});

	it('lists rules on the rules tab', () => {
		setPageUrl('http://localhost/alerts?tab=rules');
		renderPage();
		expect(screen.getByText('API memory')).toBeInTheDocument();
		expect(screen.getByText(/memory usage above 90.0% for 5 min/i)).toBeInTheDocument();
		expect(screen.getByText('Crash loops')).toBeInTheDocument();
	});
});
