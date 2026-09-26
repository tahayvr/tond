import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RequestMetricsResponse } from '$lib/api';
import RequestMetricsPanel from './request-metrics-panel.svelte';

const api = vi.hoisted(() => ({ getRequestMetrics: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

function response(overrides: Partial<RequestMetricsResponse> = {}): RequestMetricsResponse {
	return {
		range: '24h',
		step: 180,
		from: 1_000_000,
		to: 1_000_540,
		series: {
			t: [1_000_000, 1_000_180, 1_000_360],
			rps: [2, 3, 4],
			rps_1xx: [0, 0, 0],
			rps_2xx: [1.8, 2.5, 3],
			rps_3xx: [0.1, 0.2, 0.2],
			rps_4xx: [0.1, 0.2, 0.3],
			rps_5xx: [0, 0.1, 0.5],
			err_pct: [0, 3.33, 12.5],
			p50: [20, 22, null],
			p95: [60, 70, null],
			p99: [120, 150, null],
			bytes_in: [100, 200, 300],
			bytes_out: [10_000, 20_000, 30_000]
		},
		summary: {
			requests: 1620,
			rps: 3,
			s1xx: 0,
			s2xx: 1300,
			s3xx: 90,
			s4xx: 108,
			s5xx: 108,
			s502: 90,
			s504: 6,
			err_pct: 6.67,
			p50: 21.5,
			p95: 66.2,
			p99: 140,
			upstream_p50: 19,
			upstream_p95: 61.4,
			upstream_p99: 130,
			bytes_in: 108_000,
			bytes_out: 10_800_000
		},
		current: { t: 1_000_540, secs: 60, rps: 4.2, err_pct: 12.5, p95: 80 },
		markers: [
			{ t: 1_000_100, kind: 'deploy', detail: 'succeeded', deployment_id: 'd1' },
			{ t: 1_000_400, kind: 'restart', detail: 'exit code 1', deployment_id: null }
		],
		proxy_connected: true,
		...overrides
	};
}

describe('RequestMetricsPanel', () => {
	beforeEach(() => {
		api.getRequestMetrics.mockReset();
	});

	it('shows totals, error share, latency and the three request charts', async () => {
		api.getRequestMetrics.mockResolvedValue(response());
		render(RequestMetricsPanel, { owner: 'apps', id: 'app-1' });

		expect(await screen.findByText('1.6k')).toBeInTheDocument();
		expect(api.getRequestMetrics).toHaveBeenCalledWith('apps', 'app-1', '24h');
		expect(screen.getByText('4.2/s now')).toBeInTheDocument();
		expect(screen.getByText('6.7%')).toBeInTheDocument();
		expect(screen.getByText(/108 5xx · 90 × 502 · 6 × 504 · 108 4xx/)).toBeInTheDocument();
		expect(screen.getByText('66.2 ms')).toBeInTheDocument();
		expect(screen.getByText('p50 21.5 ms · p99 140 ms')).toBeInTheDocument();
		expect(screen.getByText('10.3 MB')).toBeInTheDocument();

		expect(screen.getAllByRole('slider')).toHaveLength(3);
		// Stacked chart: the tooltip text reads each status class, not the running total.
		expect(screen.getByRole('slider', { name: /requests per second by status/i })).toHaveAttribute(
			'aria-valuetext',
			expect.stringContaining('5xx 0.50/s')
		);
		// Buckets without requests read as "—", not 0 ms.
		expect(screen.getByRole('slider', { name: /response time/i })).toHaveAttribute(
			'aria-valuetext',
			expect.stringContaining('p95 —')
		);

		const timeline = screen.getByText('Deploys and restarts').closest('[data-slot="card"]');
		expect(within(timeline as HTMLElement).getByText('Deployed')).toBeInTheDocument();
		expect(
			within(timeline as HTMLElement).getByRole('link', { name: 'View deploy' })
		).toHaveAttribute('href', '/apps/app-1/deployments/d1');
	});

	it('refetches on range change and explains an empty window', async () => {
		api.getRequestMetrics.mockResolvedValue(
			response({
				summary: { ...response().summary, requests: 0 },
				markers: [],
				current: null
			})
		);
		const user = userEvent.setup();
		render(RequestMetricsPanel, { owner: 'apps', id: 'app-1' });
		expect(await screen.findByText('No requests in the last 24 hours')).toBeInTheDocument();
		expect(screen.getByText(/background workers, have no request metrics/)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: '7d' }));
		expect(api.getRequestMetrics).toHaveBeenLastCalledWith('apps', 'app-1', '7d');
		expect(await screen.findByText('No requests in the last 7 days')).toBeInTheDocument();
	});

	it('warns when the proxy is not answering and offers a retry on errors', async () => {
		api.getRequestMetrics.mockResolvedValueOnce(response({ proxy_connected: false }));
		render(RequestMetricsPanel, { owner: 'previews', id: 'pv-1', noun: 'preview' });
		expect(await screen.findByText(/tond-proxy isn't answering/)).toBeInTheDocument();
		// Previews have no deploy pages to link to.
		expect(screen.queryByRole('link', { name: 'View deploy' })).toBeNull();
	});

	it('shows load errors with a retry', async () => {
		api.getRequestMetrics.mockRejectedValueOnce(new Error('boom'));
		api.getRequestMetrics.mockResolvedValueOnce(response());
		const user = userEvent.setup();
		render(RequestMetricsPanel, { owner: 'apps', id: 'app-1' });
		expect(await screen.findByText('boom')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Retry' }));
		expect(await screen.findByText('1.6k')).toBeInTheDocument();
	});
});
