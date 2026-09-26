import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { MetricsResponse } from '$lib/api';
import MetricsPanel from './metrics-panel.svelte';

const api = vi.hoisted(() => ({ getMetrics: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const MB = 1024 * 1024;

function response(overrides: Partial<MetricsResponse> = {}): MetricsResponse {
	return {
		range: '1h',
		step: 30,
		from: 1_000_000,
		to: 1_003_600,
		mem_limit: 512 * MB,
		series: {
			t: [1_000_000, 1_000_030, 1_000_060],
			cpu: [10, 20, 30],
			cpu_max: [15, 25, 60],
			mem: [100 * MB, 110 * MB, 120 * MB],
			mem_max: [100 * MB, 110 * MB, 130 * MB],
			rx: [1024, 2048, 4096],
			tx: [512, 512, 512],
			pids: [3, 3, 3]
		},
		events: [
			{ t: 1_000_030, kind: 'restart', detail: 'exit code 137' },
			{ t: 1_000_040, kind: 'oom', detail: '1 process killed' }
		],
		current: {
			t: 1_000_060,
			cpu: 30,
			mem: 120 * MB,
			mem_peak: 300 * MB,
			rx: 4096,
			tx: 512,
			pids: 3
		},
		...overrides
	};
}

describe('MetricsPanel', () => {
	beforeEach(() => {
		api.getMetrics.mockReset();
	});

	it('shows current values, separate charts and the restart timeline', async () => {
		api.getMetrics.mockResolvedValue(response());
		render(MetricsPanel, { owner: 'apps', id: 'app-1' });

		expect(await screen.findByText('30.0%')).toBeInTheDocument();
		expect(api.getMetrics).toHaveBeenCalledWith('apps', 'app-1', '1h');
		expect(screen.getByText('120 MB')).toBeInTheDocument();
		expect(screen.getByText(/of 512 MB limit · peak 300 MB/)).toBeInTheDocument();
		expect(screen.getByText('Network now').nextElementSibling).toHaveTextContent('4.0 KB/s');
		// One chart per measure, each keyboard-readable.
		expect(screen.getAllByRole('slider')).toHaveLength(3);
		expect(screen.getByRole('slider', { name: /network traffic/i })).toHaveAttribute(
			'aria-valuetext',
			expect.stringContaining('in 4.0 KB/s, out 512 B/s')
		);
		const timeline = screen.getByText('Restarts and crashes').closest('[data-slot="card"]');
		expect(timeline).not.toBeNull();
		expect(within(timeline as HTMLElement).getByText('exit code 137')).toBeInTheDocument();
		expect(within(timeline as HTMLElement).getByText('Out of memory')).toBeInTheDocument();
		expect(screen.getByText('1 out-of-memory kill')).toBeInTheDocument();
	});

	it('refetches when the range changes', async () => {
		api.getMetrics.mockResolvedValue(response());
		const user = userEvent.setup();
		render(MetricsPanel, { owner: 'databases', id: 'db-1', noun: 'database' });
		await screen.findByText('30.0%');

		api.getMetrics.mockResolvedValue(response({ range: '7d', step: 1500 }));
		await user.click(screen.getByRole('button', { name: '7d' }));
		await vi.waitFor(() =>
			expect(api.getMetrics).toHaveBeenLastCalledWith('databases', 'db-1', '7d')
		);
		expect(screen.getByRole('button', { name: '7d' })).toHaveAttribute('aria-pressed', 'true');
		expect(await screen.findByText(/25 min averages/)).toBeInTheDocument();
	});

	it('explains an empty range', async () => {
		api.getMetrics.mockResolvedValue(
			response({
				series: { t: [], cpu: [], cpu_max: [], mem: [], mem_max: [], rx: [], tx: [], pids: [] },
				events: [],
				current: null
			})
		);
		render(MetricsPanel, { owner: 'apps', id: 'app-1' });
		expect(await screen.findByText('No data for the last hour')).toBeInTheDocument();
		expect(screen.getByText(/while this app is running/)).toBeInTheDocument();
	});

	it('shows a retryable error', async () => {
		api.getMetrics.mockRejectedValueOnce(new Error('boom'));
		api.getMetrics.mockResolvedValue(response());
		const user = userEvent.setup();
		render(MetricsPanel, { owner: 'apps', id: 'app-1' });
		expect(await screen.findByRole('alert')).toHaveTextContent("Couldn't load metrics: boom");
		await user.click(screen.getByRole('button', { name: 'Retry' }));
		expect(await screen.findByText('30.0%')).toBeInTheDocument();
		expect(screen.queryByRole('alert')).toBeNull();
	});
});
