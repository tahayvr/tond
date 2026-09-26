import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AuditShipping, DeliveryStatus } from '$lib/api';
import AuditShippingForm from './audit-shipping-form.svelte';

const api = vi.hoisted(() => ({
	getAuditShipping: vi.fn(),
	updateAuditShipping: vi.fn(),
	testAuditShipping: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const idle: DeliveryStatus = {
	shipped_seq: 0,
	pending: 3,
	failures: 0,
	last_attempt_at: null,
	last_success_at: null,
	last_error: null,
	next_attempt_at: null,
	dropped: 0
};

function shipping(): AuditShipping {
	return {
		syslog: { enabled: false, host: '', port: 6514, tls: true, status: idle },
		webhook: {
			enabled: false,
			url: 'https://logs.example.com/x',
			has_secret: false,
			status: idle
		},
		latest_seq: 3
	};
}

describe('AuditShippingForm', () => {
	beforeEach(() => {
		api.getAuditShipping.mockReset().mockResolvedValue(shipping());
		api.updateAuditShipping.mockReset().mockResolvedValue(shipping());
		api.testAuditShipping.mockReset().mockResolvedValue({
			ok: false,
			message: 'the webhook answered HTTP 401'
		});
	});

	it('saves the webhook with a generated secret', async () => {
		render(AuditShippingForm);
		await screen.findByLabelText('URL');
		await userEvent.click(screen.getByLabelText('HTTPS webhook'));
		await userEvent.click(screen.getByRole('button', { name: 'Generate' }));
		expect(screen.getByText(/never shown again/)).toBeTruthy();
		const saves = screen.getAllByRole('button', { name: 'Save' });
		await userEvent.click(saves[1]);
		await waitFor(() => expect(api.updateAuditShipping).toHaveBeenCalled());
		const body = api.updateAuditShipping.mock.calls[0][0];
		expect(body.webhook.enabled).toBe(true);
		expect(body.webhook.url).toBe('https://logs.example.com/x');
		expect(body.webhook.secret).toMatch(/^[0-9a-f]{64}$/);
		expect(body.syslog).toBeUndefined();
	});

	it('shows the result of a test delivery', async () => {
		render(AuditShippingForm);
		await screen.findByLabelText('URL');
		const tests = screen.getAllByRole('button', { name: 'Send test' });
		await userEvent.click(tests[1]);
		expect(await screen.findByText('the webhook answered HTTP 401')).toBeTruthy();
		expect(api.testAuditShipping).toHaveBeenCalledWith('webhook');
	});

	it('checks the syslog port before saving', async () => {
		render(AuditShippingForm);
		const port = await screen.findByLabelText('Port');
		await userEvent.clear(port);
		await userEvent.type(port, '70000');
		await userEvent.click(screen.getAllByRole('button', { name: 'Save' })[0]);
		expect(await screen.findByText('The port must be between 1 and 65535.')).toBeTruthy();
		expect(api.updateAuditShipping).not.toHaveBeenCalled();
	});
});
