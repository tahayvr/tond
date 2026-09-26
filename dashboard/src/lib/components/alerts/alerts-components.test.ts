import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ActiveAlert } from '$lib/api';
import {
	MOCK_ACTIVE_ALERTS,
	MOCK_ALERT_DELIVERIES,
	MOCK_ALERT_HISTORY
} from '../../../mocks/fixtures';

const api = vi.hoisted(() => ({
	listAlertHistory: vi.fn(),
	getAlertEpisode: vi.fn()
}));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	...api
}));

import FiringBanner from './FiringBanner.svelte';
import AlertHistory from './AlertHistory.svelte';
import AlertEpisodeDialog from './AlertEpisodeDialog.svelte';

const firing = (MOCK_ACTIVE_ALERTS as ActiveAlert[]).filter((a) => a.state === 'firing');

describe('FiringBanner', () => {
	it('renders nothing when nothing is firing', () => {
		const { container } = render(FiringBanner, { props: { alerts: [] } });
		expect(container.textContent?.trim()).toBe('');
	});

	it('names the firing alerts and links to them', () => {
		render(FiringBanner, { props: { alerts: firing } });
		expect(screen.getByRole('heading', { name: '2 alerts are firing' })).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'API memory · api-service' })).toHaveAttribute(
			'href',
			'/alerts?alert=ep_1'
		);
		expect(screen.getByRole('link', { name: /open alerts/i })).toHaveAttribute('href', '/alerts');
	});
});

describe('AlertHistory', () => {
	beforeEach(() => {
		api.listAlertHistory.mockReset();
		api.listAlertHistory.mockImplementation(async (f: { page: number; per_page: number }) => ({
			episodes: MOCK_ALERT_HISTORY.slice((f.page - 1) * f.per_page, f.page * f.per_page),
			total: MOCK_ALERT_HISTORY.length
		}));
	});

	it('pages through older alerts', async () => {
		const user = userEvent.setup();
		render(AlertHistory, { props: { rules: [], apps: [] } });
		expect(await screen.findByText(/page 1 of 2/i)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Older' }));
		expect(await screen.findByText(/page 2 of 2/i)).toBeInTheDocument();
		expect(api.listAlertHistory).toHaveBeenLastCalledWith(
			expect.objectContaining({ page: 2, per_page: 20 })
		);
	});

	it('limits to one rule when given', async () => {
		render(AlertHistory, { props: { ruleId: 'alert_2' } });
		await screen.findAllByText(/apps down/i);
		expect(api.listAlertHistory).toHaveBeenCalledWith(
			expect.objectContaining({ rule_id: 'alert_2', page: 1 })
		);
		expect(screen.queryByRole('combobox', { name: 'Rule' })).not.toBeInTheDocument();
	});
});

describe('AlertEpisodeDialog', () => {
	it('shows every notification and why one failed', async () => {
		api.getAlertEpisode.mockResolvedValue({
			episode: MOCK_ALERT_HISTORY[0],
			deliveries: MOCK_ALERT_DELIVERIES.ep_1
		});
		render(AlertEpisodeDialog, { props: { episodeId: 'ep_1' } });
		expect(await screen.findByText(/HTTP 502 from hooks.example.com/)).toBeInTheDocument();
		expect(screen.getByText('Failed')).toBeInTheDocument();
		expect(screen.getByText(/gave up after 6 attempts/i)).toBeInTheDocument();
		expect(screen.getByText('Sent')).toBeInTheDocument();
	});
});
