import { render, screen, within } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { makeMockSystemSelf } from '../../mocks/fixtures';

const api = vi.hoisted(() => ({ getSystemSelf: vi.fn() }));
vi.mock('$lib/api', () => api);

import TondHealthPanel from '$lib/components/system/TondHealthPanel.svelte';

describe('Tond health panel', () => {
	beforeEach(() => {
		api.getSystemSelf.mockReset();
	});

	it("shows Tond's memory, CPU, proxy link, database and background loops", async () => {
		const self = makeMockSystemSelf();
		self.loops[2] = { ...self.loops[2], errors: 2, last_error: 'database is locked' };
		api.getSystemSelf.mockResolvedValue(self);

		render(TondHealthPanel);

		expect(await screen.findByText('13.4 MB')).toBeInTheDocument();
		const server = screen.getByText('tond-server').closest('tr') as HTMLElement;
		expect(within(server).getByText(/CPU <0\.1%/)).toBeInTheDocument();
		const proxy = screen.getByText('tond-proxy').closest('tr') as HTMLElement;
		expect(within(proxy).getByText('Connected')).toBeInTheDocument();
		expect(within(proxy).getByText(/route changes apply instantly/)).toBeInTheDocument();
		expect(screen.getByText('1.8 MB')).toBeInTheDocument();

		const health = screen.getByText('Health checks').closest('tr');
		expect(health).not.toBeNull();
		expect(within(health as HTMLElement).getByLabelText('Healthy')).toBeInTheDocument();

		const alerts = screen.getByText('Alert rules').closest('tr') as HTMLElement;
		expect(within(alerts).getByText('database is locked')).toBeInTheDocument();
		expect(within(alerts).getByLabelText('Last run failed')).toBeInTheDocument();
		expect(within(alerts).getByText('on change')).toBeInTheDocument();
		expect(screen.getByText(/1 failing/)).toBeInTheDocument();

		// Proxy loops are listed and tagged.
		const poll = screen.getByText('Route fallback check').closest('tr') as HTMLElement;
		expect(within(poll).getByText('proxy')).toBeInTheDocument();
	});

	it('says when the proxy is not connected', async () => {
		const self = makeMockSystemSelf();
		api.getSystemSelf.mockResolvedValue({
			...self,
			proxy: { connected: false, stats: null, cpu_percent: null }
		});

		render(TondHealthPanel);

		expect(await screen.findByText('Not connected')).toBeInTheDocument();
		expect(screen.getByText(/not connected: routes update within a minute/)).toBeInTheDocument();
	});

	it('lists Docker and the heartbeat with what to do when they fail', async () => {
		api.getSystemSelf.mockResolvedValue(makeMockSystemSelf());

		render(TondHealthPanel, {
			docker: {
				reachable: false,
				version: null,
				api_version: null,
				storage_driver: null,
				root_dir: null,
				cgroup_version: null,
				warnings: [],
				containers: null,
				containers_running: null,
				images: null,
				rss_bytes: null,
				uptime_secs: null,
				error: 'Docker did not answer within 5 s',
				checked_at: 0
			},
			heartbeat: {
				configured: true,
				url_host: 'hc-ping.com',
				method: 'GET',
				interval_secs: 300,
				enabled: true,
				last_ok_at: 100,
				last_failure_at: null,
				last_error: null,
				last_skipped_at: 400,
				last_skip_reason: 'Docker is not healthy',
				next_at: null,
				healthy: false,
				unhealthy_reason: 'Docker is not healthy'
			}
		});

		const docker = (await screen.findByText('Docker')).closest('tr') as HTMLElement;
		expect(within(docker).getByText('Unreachable')).toBeInTheDocument();
		expect(within(docker).getByText(/systemctl status docker/)).toBeInTheDocument();
		const beat = screen.getByText('Heartbeat').closest('tr') as HTMLElement;
		expect(within(beat).getByText('Holding')).toBeInTheDocument();
		expect(within(beat).getByText(/Docker is not healthy/)).toBeInTheDocument();
	});

	it('explains a failed load', async () => {
		api.getSystemSelf.mockRejectedValue(new Error('Service unavailable'));

		render(TondHealthPanel);

		expect(await screen.findByRole('alert')).toHaveTextContent(
			"Couldn't load Tond's own health: Service unavailable. It retries every 10 s."
		);
	});
});
