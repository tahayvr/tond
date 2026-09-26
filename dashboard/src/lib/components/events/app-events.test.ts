import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEvent } from '$lib/api';
import AppEventsStrip from './app-events-strip.svelte';
import AppEventsList from './app-events-list.svelte';

const api = vi.hoisted(() => ({ getAppEvents: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const now = Date.now();
const deploy: AppEvent = {
	at: new Date(now - 60_000).toISOString(),
	kind: 'deploy_succeeded',
	severity: 'success',
	title: 'Deploy succeeded in 42 s',
	detail: 'abc1234 Fix login',
	actor: 'dev@example.com',
	deployment_id: 'd1'
};
const oom: AppEvent = {
	at: new Date(now - 3_600_000).toISOString(),
	kind: 'oom',
	severity: 'danger',
	title: 'Killed: out of memory',
	detail: 'exit code 137',
	actor: null,
	deployment_id: null
};

describe('AppEventsStrip', () => {
	beforeEach(() => {
		api.getAppEvents.mockReset().mockResolvedValue({ events: [deploy, oom], next_before: null });
	});

	it('shows recent events, links deploys and counts problems', async () => {
		render(AppEventsStrip, { appId: 'a1' });
		const link = await screen.findByRole('link', { name: 'Deploy succeeded in 42 s' });
		expect(link.getAttribute('href')).toBe('/apps/a1/deployments/d1');
		expect(screen.getByText('Killed: out of memory')).toBeTruthy();
		expect(screen.getByText(/1\s+problem/)).toBeTruthy();
		expect(screen.getByRole('link', { name: /All events/ }).getAttribute('href')).toBe(
			'/apps/a1?tab=events'
		);
		expect(api.getAppEvents).toHaveBeenCalledWith('a1', { limit: 5 });
	});

	it('explains an empty feed', async () => {
		api.getAppEvents.mockResolvedValue({ events: [], next_before: null });
		render(AppEventsStrip, { appId: 'a1' });
		expect(await screen.findByText(/Nothing yet/)).toBeTruthy();
	});
});

describe('AppEventsList', () => {
	beforeEach(() => {
		api.getAppEvents
			.mockReset()
			.mockResolvedValueOnce({ events: [deploy], next_before: deploy.at })
			.mockResolvedValueOnce({ events: [oom], next_before: null });
	});

	it('filters and loads older events', async () => {
		render(AppEventsList, { appId: 'a1' });
		await screen.findByText('Deploy succeeded in 42 s');
		await userEvent.click(screen.getByRole('button', { name: 'Load older events' }));
		expect(await screen.findByText('Killed: out of memory')).toBeTruthy();
		expect(api.getAppEvents).toHaveBeenLastCalledWith('a1', { limit: 50, before: deploy.at });

		await userEvent.click(screen.getByRole('button', { name: 'Health' }));
		expect(screen.queryByText('Deploy succeeded in 42 s')).toBeNull();
		expect(screen.getByText('Killed: out of memory')).toBeTruthy();
	});
});
