import { render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TrafficOverview as Overview } from '$lib/api';
import TrafficOverview from './traffic-overview.svelte';

const api = vi.hoisted(() => ({ getTrafficOverview: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const summary = {
	requests: 12_500,
	rps: 0.14,
	s1xx: 0,
	s2xx: 12_000,
	s3xx: 0,
	s4xx: 300,
	s5xx: 200,
	s502: 200,
	s504: 0,
	err_pct: 1.6,
	p50: 20,
	p95: 75,
	p99: 190,
	upstream_p50: 18,
	upstream_p95: 70,
	upstream_p99: 180,
	bytes_in: 1,
	bytes_out: 2
};

function overview(overrides: Partial<Overview> = {}): Overview {
	const web = {
		id: 'a1',
		name: 'web',
		requests: 10_000,
		rps: 0.12,
		s4xx: 200,
		s5xx: 0,
		err_pct: 0,
		p95: 60,
		bytes_out: 1
	};
	const api = { ...web, id: 'a2', name: 'api', requests: 2_000, s5xx: 200, err_pct: 10 };
	return {
		range: '24h',
		from: 0,
		to: 86_400,
		total: summary,
		unknown_requests: 500,
		busiest: [web, api],
		errors: [api],
		proxy_connected: true,
		...overrides
	};
}

describe('TrafficOverview', () => {
	beforeEach(() => api.getTrafficOverview.mockReset());

	it('lists the busiest apps and the ones answering with 5xx', async () => {
		api.getTrafficOverview.mockResolvedValue(overview());
		render(TrafficOverview);
		expect(await screen.findByText('12.5k')).toBeInTheDocument();
		expect(api.getTrafficOverview).toHaveBeenCalledWith('24h');
		expect(screen.getByText('1.6%')).toHaveClass('text-destructive');

		const busiest = screen.getByText('Busiest apps').parentElement as HTMLElement;
		const links = within(busiest).getAllByRole('link');
		expect(links.map((l) => l.textContent)).toEqual(['web', 'api']);
		expect(links[0]).toHaveAttribute('href', '/apps/a1?tab=metrics');

		const errors = screen.getByText('Server errors').parentElement as HTMLElement;
		expect(within(errors).getByText('200 × 5xx · 10%')).toBeInTheDocument();
		expect(screen.getByText(/500 requests for hosts Tond doesn't serve/)).toBeInTheDocument();
	});

	it('explains an idle server and a quiet error list', async () => {
		api.getTrafficOverview.mockResolvedValueOnce(
			overview({ total: { ...summary, requests: 0 }, busiest: [], errors: [] })
		);
		render(TrafficOverview);
		expect(await screen.findByText(/No requests yet/)).toBeInTheDocument();
	});

	it('shows no errors reassurance', async () => {
		api.getTrafficOverview.mockResolvedValueOnce(overview({ errors: [], unknown_requests: 0 }));
		render(TrafficOverview);
		expect(await screen.findByText('No app answered with a 5xx.')).toBeInTheDocument();
		expect(screen.queryByText(/hosts Tond doesn't serve/)).toBeNull();
	});
});
