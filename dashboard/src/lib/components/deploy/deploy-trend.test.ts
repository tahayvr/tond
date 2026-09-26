import { render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import DeployTrend from './deploy-trend.svelte';

const api = vi.hoisted(() => ({ getDeployStats: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

describe('DeployTrend', () => {
	it('shows the success rate, median duration and one cell per deploy', async () => {
		api.getDeployStats.mockResolvedValue({
			window: 3,
			succeeded: 2,
			failed: 1,
			success_rate: 2 / 3,
			median_duration_secs: 72,
			outcomes: [
				{
					id: 'd1',
					status: 'succeeded',
					started_at: '2026-09-01T00:00:00Z',
					duration_secs: 60,
					commit_sha: 'abcdef1234'
				},
				{
					id: 'd2',
					status: 'failed',
					started_at: '2026-09-02T00:00:00Z',
					duration_secs: 30,
					commit_sha: null
				},
				{
					id: 'd3',
					status: 'succeeded',
					started_at: '2026-09-03T00:00:00Z',
					duration_secs: 84,
					commit_sha: null
				}
			]
		});
		render(DeployTrend, { appId: 'a1' });
		expect(await screen.findByText('67%')).toBeTruthy();
		expect(screen.getByText('2 of 3, last 3 deploys')).toBeTruthy();
		const cells = screen.getAllByRole('link');
		expect(cells).toHaveLength(3);
		expect(cells[0].getAttribute('href')).toBe('/apps/a1/deployments/d1');
		expect(cells[0].getAttribute('aria-label')).toContain('abcdef1 succeeded');
		expect(cells[1].getAttribute('aria-label')).toContain('failed');
	});

	it('renders nothing before the first finished deploy', async () => {
		api.getDeployStats.mockResolvedValue({
			window: 0,
			succeeded: 0,
			failed: 0,
			success_rate: null,
			median_duration_secs: null,
			outcomes: []
		});
		const { container } = render(DeployTrend, { appId: 'a1' });
		await Promise.resolve();
		expect(container.querySelector('section')).toBeNull();
	});
});
