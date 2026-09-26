import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Integration } from '$lib/api';
import { MOCK_ALERT_KINDS } from '../../mocks/fixtures';

const { goto, createAlertRule } = vi.hoisted(() => ({
	goto: vi.fn(),
	createAlertRule: vi.fn()
}));

vi.mock('$app/navigation', () => ({ goto }));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	createAlertRule
}));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	alertsContext: vi.fn(() => ({ type: 'alerts' }))
}));

import NewAlertPage from '../../routes/alerts/new/+page.svelte';

function channel(id: string, name: string, kind: Integration['kind'], enabled = true): Integration {
	return {
		id,
		name,
		kind,
		enabled,
		config: {} as Integration['config'],
		saved_secrets: [],
		quiet_hours: { mode: 'global' },
		last_sent_at: null,
		last_error: null,
		last_error_at: null,
		created_at: '',
		updated_at: ''
	};
}

const integrations: Integration[] = [
	channel('int_1', 'Phone', 'ntfy'),
	{ ...channel('int_2', 'Old email', 'email', false), last_error: 'SMTP send error: refused' }
];

const data = (integrations: Integration[]) => ({
	kinds: MOCK_ALERT_KINDS,
	apps: [],
	integrations,
	quietHours: {
		enabled: true,
		start: '22:00',
		end: '07:00',
		timezone: 'Europe/Berlin',
		critical_bypass: true
	}
});

describe('alerts/new page', () => {
	beforeEach(() => {
		goto.mockReset();
		createAlertRule.mockReset().mockResolvedValue({ id: 'alert_9' });
	});

	it('requires a channel and sends the kind defaults with the chosen channels', async () => {
		const user = userEvent.setup();
		render(NewAlertPage, { props: { data: data(integrations), params: {} } });

		// The first kind is "App down": critical, fires after 2 minutes.
		expect(screen.getByText(/the app is not running/i)).toBeInTheDocument();
		await user.type(screen.getByLabelText('Name'), 'Apps down');
		const create = screen.getByRole('button', { name: /create rule/i });
		expect(create).toBeDisabled();
		expect(screen.getByLabelText(/old email/i)).toBeDisabled();
		expect(screen.getByText(/last delivery failed: smtp send error/i)).toBeInTheDocument();
		expect(screen.getAllByText(/quiet 22:00–07:00 europe\/berlin \(global\)/i)).toHaveLength(2);

		await user.click(screen.getByLabelText(/phone/i));
		expect(create).toBeEnabled();
		await user.click(create);

		expect(createAlertRule).toHaveBeenCalledWith({
			name: 'Apps down',
			kind: 'app_down',
			app_id: null,
			for_secs: 120,
			severity: 'critical',
			repeat_secs: 14_400,
			integration_ids: ['int_1']
		});
		expect(goto).toHaveBeenCalledWith('/alerts/alert_9', { invalidateAll: true });
	});

	it('points to the Integrations page when there is no channel yet', () => {
		render(NewAlertPage, { props: { data: data([]), params: {} } });
		expect(screen.getByText(/no notification channels yet/i)).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /add one/i })).toHaveAttribute('href', '/integrations');
	});

	it('shows a save error next to the form', async () => {
		createAlertRule.mockRejectedValue(new Error('Threshold must be between 0% and 100%.'));
		const user = userEvent.setup();
		render(NewAlertPage, { props: { data: data(integrations), params: {} } });
		await user.type(screen.getByLabelText('Name'), 'x');
		await user.click(screen.getByLabelText(/phone/i));
		await user.click(screen.getByRole('button', { name: /create rule/i }));
		expect(await screen.findByRole('alert')).toHaveTextContent(/between 0% and 100%/);
		expect(goto).not.toHaveBeenCalled();
	});
});
