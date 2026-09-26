import { render, screen } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { makeMockMetrics, makeMockRequestMetrics } from '../../../../mocks/fixtures';
import AppOverviewMetrics from './app-overview-metrics.svelte';

const api = vi.hoisted(() => ({ getRequestMetrics: vi.fn(), getMetrics: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

describe('AppOverviewMetrics', () => {
	beforeEach(() => {
		api.getRequestMetrics.mockReset();
		api.getMetrics.mockReset();
	});

	it('shows five sparklines for the last 24 hours and links to the metrics tab', async () => {
		const requests = makeMockRequestMetrics('24h');
		requests.current = { t: requests.to, secs: 60, rps: 3.4, err_pct: 0, p95: 50 };
		requests.summary.err_pct = 2.5;
		api.getRequestMetrics.mockResolvedValue(requests);
		api.getMetrics.mockResolvedValue(makeMockMetrics('24h'));
		render(AppOverviewMetrics, { appId: 'a1' });

		expect(await screen.findByText('3.4/s')).toBeInTheDocument();
		expect(api.getRequestMetrics).toHaveBeenCalledWith('apps', 'a1', '24h');
		expect(api.getMetrics).toHaveBeenCalledWith('apps', 'a1', '24h');
		expect(screen.getAllByTestId('sparkline')).toHaveLength(5);
		// Errors above 1% are highlighted.
		expect(screen.getByText('2.5%').closest('[data-testid="sparkline"]')).toHaveClass('bg-warn/5');
		expect(screen.getByRole('link', { name: /all metrics/i })).toHaveAttribute(
			'href',
			'/apps/a1?tab=metrics'
		);
	});

	it('keeps working when one source fails and says so when both do', async () => {
		api.getRequestMetrics.mockRejectedValueOnce(new Error('no proxy'));
		api.getMetrics.mockResolvedValueOnce(makeMockMetrics('24h'));
		const { unmount } = render(AppOverviewMetrics, { appId: 'a1' });
		expect(await screen.findByText('Latency p95')).toBeInTheDocument();
		expect(screen.getAllByTestId('sparkline')).toHaveLength(5);
		unmount();

		api.getRequestMetrics.mockRejectedValueOnce(new Error('down'));
		api.getMetrics.mockRejectedValueOnce(new Error('down'));
		render(AppOverviewMetrics, { appId: 'a1' });
		expect(await screen.findByText(/Metrics are unavailable right now/)).toBeInTheDocument();
	});
});
