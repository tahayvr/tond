import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Integration } from '$lib/api';

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

const integrations: Integration[] = [
	{
		id: 'int_1',
		name: 'Ops Slack',
		kind: 'slack' as const,
		enabled: true,
		config: {} as Integration['config'],
		saved_secrets: [],
		created_at: '',
		updated_at: ''
	},
	{
		id: 'int_2',
		name: 'Old email',
		kind: 'email' as const,
		enabled: false,
		config: {} as Integration['config'],
		saved_secrets: [],
		created_at: '',
		updated_at: ''
	}
];

describe('alerts/new page', () => {
	beforeEach(() => {
		goto.mockReset();
		createAlertRule.mockReset().mockResolvedValue({ id: 'alert_9' });
	});

	it('requires a channel or webhook, and links the chosen channels on creation', async () => {
		const user = userEvent.setup();
		render(NewAlertPage, { props: { data: { apps: [], integrations }, params: {} } });

		await user.type(screen.getByLabelText('Name'), 'CPU high');
		const create = screen.getByRole('button', { name: /create alert rule/i });
		expect(create).toBeDisabled();
		expect(screen.getByLabelText(/old email/i)).toBeDisabled();

		await user.click(screen.getByLabelText(/ops slack/i));
		expect(create).toBeEnabled();
		await user.click(create);

		expect(createAlertRule).toHaveBeenCalledWith(
			expect.objectContaining({ name: 'CPU high', integration_ids: ['int_1'], webhook_url: '' })
		);
		expect(goto).toHaveBeenCalledWith('/alerts/alert_9', { invalidateAll: true });
	});

	it('accepts a webhook on its own', async () => {
		const user = userEvent.setup();
		render(NewAlertPage, { props: { data: { apps: [], integrations: [] }, params: {} } });

		expect(screen.getByText(/no notification channels yet/i)).toBeInTheDocument();
		await user.type(screen.getByLabelText('Name'), 'Deploys');
		await user.type(screen.getByLabelText(/webhook url/i), 'https://hooks.example.com/x');
		await user.click(screen.getByRole('button', { name: /create alert rule/i }));

		expect(createAlertRule).toHaveBeenCalledWith(
			expect.objectContaining({ webhook_url: 'https://hooks.example.com/x', integration_ids: [] })
		);
	});
});
