import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { UptimeCheck } from '$lib/api';
import UptimePanel from './UptimePanel.svelte';

const api = vi.hoisted(() => ({
	getUptime: vi.fn(),
	checkUptimeNow: vi.fn(),
	updateUptime: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const now = Math.floor(Date.now() / 1000);

function check(over: Partial<UptimeCheck> = {}): UptimeCheck {
	return {
		check: {
			id: 7,
			enabled: true,
			path: '/',
			interval_secs: 60,
			expected_status: null,
			timeout_secs: 10
		},
		host: 'web.example.com',
		url: 'https://web.example.com/',
		status: 'down',
		paused_reason: null,
		last: {
			at: now - 20,
			ok: false,
			cause: 'http_5xx',
			detail: "HTTP 502: the proxy couldn't reach the app.",
			status: 502,
			latency_ms: 12,
			ip: '203.0.113.5',
			dns: 'matches',
			cert_expires_at: now + 30 * 86_400,
			cert_days_left: 30,
			dns_label: 'Points at this server'
		},
		down_since: now - 300,
		next_at: now + 40,
		uptime: { day: 99.5, week: 99.9, month: 99.97, day_probes: 1440 },
		recent: [{ t: now - 1800, probes: 30, up: 25 }],
		daily: [{ t: now - 86_400, probes: 1440, up: 1435 }],
		latency: { t: [], ms: [] },
		incidents: [
			{
				id: 1,
				check_id: 7,
				started_at: now - 300,
				ended_at: null,
				cause: 'http_5xx',
				detail: "HTTP 502: the proxy couldn't reach the app.",
				duration_secs: 300
			}
		],
		...over
	};
}

describe('UptimePanel', () => {
	beforeEach(() => vi.clearAllMocks());

	it('shows why a check is down, its numbers and the incident', async () => {
		api.getUptime.mockResolvedValue(check());
		render(UptimePanel, { props: { target: 'web', canEdit: true } });
		expect(await screen.findByText('Down for 5 min')).toBeTruthy();
		expect(screen.getAllByText('Server error (5xx)').length).toBeGreaterThan(0);
		expect(screen.getByText('99.50%')).toBeTruthy();
		expect(screen.getByText('30 days left')).toBeTruthy();
		expect(screen.getByText(/ongoing/)).toBeTruthy();
	});

	it('checks now and saves validated settings', async () => {
		const user = userEvent.setup();
		api.getUptime.mockResolvedValue(check());
		api.checkUptimeNow.mockResolvedValue(
			check({
				status: 'up',
				down_since: null,
				incidents: [],
				last: { ...check().last!, ok: true, status: 200, cause: null, detail: 'HTTP 200' }
			})
		);
		api.updateUptime.mockImplementation(async (_t, u) => check({ check: { id: 7, ...u } }));
		render(UptimePanel, { props: { target: 'web', canEdit: true } });
		await user.click(await screen.findByRole('button', { name: /check now/i }));
		expect(api.checkUptimeNow).toHaveBeenCalledWith('web');
		expect(await screen.findByText('Up')).toBeTruthy();

		const path = screen.getByLabelText('Path');
		await user.clear(path);
		await user.type(path, 'health');
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(await screen.findByText(/must start with \//)).toBeTruthy();
		expect(api.updateUptime).not.toHaveBeenCalled();

		await user.clear(path);
		await user.type(path, '/health');
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(api.updateUptime).toHaveBeenCalledWith('web', {
			enabled: true,
			path: '/health',
			interval_secs: 60,
			expected_status: null,
			timeout_secs: 10
		});
		expect(await screen.findByText(/next check uses these settings/)).toBeTruthy();
	});

	it('explains a missing check', async () => {
		api.getUptime.mockResolvedValue(
			check({ check: null, status: 'paused', paused_reason: 'The app has no domain yet.' })
		);
		render(UptimePanel, { props: { target: 'worker', canEdit: true } });
		expect(await screen.findByText('The app has no domain yet.')).toBeTruthy();
	});
});
