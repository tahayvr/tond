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
		expect(screen.getByText('<0.1%')).toBeInTheDocument();
		expect(screen.getByText('Connected: route changes apply instantly')).toBeInTheDocument();
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

		expect(
			await screen.findByText('Not connected: routes update within a minute')
		).toBeInTheDocument();
		expect(screen.getByText('Not reporting')).toBeInTheDocument();
	});

	it('explains a failed load', async () => {
		api.getSystemSelf.mockRejectedValue(new Error('Service unavailable'));

		render(TondHealthPanel);

		expect(await screen.findByRole('alert')).toHaveTextContent(
			"Couldn't load Tond's own health: Service unavailable. It retries every 10 s."
		);
	});
});
