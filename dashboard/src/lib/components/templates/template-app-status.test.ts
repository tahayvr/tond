import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { TemplateAppStatus } from '$lib/api';
import { MOCK_APPS } from '../../../mocks/fixtures';

const api = vi.hoisted(() => ({
	getAppTemplate: vi.fn(),
	upgradeAppTemplate: vi.fn(),
	syncTemplateDomainEnv: vi.fn()
}));
vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
const confirm = vi.hoisted(() => ({ confirmAction: vi.fn() }));
vi.mock('$lib/confirm.svelte', () => ({ confirmAction: confirm.confirmAction }));
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));
const nav = vi.hoisted(() => ({ goto: vi.fn() }));
vi.mock('$app/navigation', () => nav);

import TemplateAppStatusView from './template-app-status.svelte';
import { domainEnvDetails } from '$lib/template-app';

const app = {
	...MOCK_APPS[0],
	id: 'app_1',
	name: 'stats',
	template_id: 'umami',
	template_version: '3.4'
};

function status(over: Partial<TemplateAppStatus> = {}): TemplateAppStatus {
	return {
		template_id: 'umami',
		template_name: 'Umami',
		version: '3.4',
		latest_version: '3.5',
		update_available: true,
		upgrade_notes: 'Back up first. 3.5 migrates the database on start.',
		upgrade_blocked: null,
		domain_env: [],
		domain_env_edited: [],
		...over
	};
}

beforeEach(() => {
	vi.clearAllMocks();
	confirm.confirmAction.mockImplementation(async (o: { onconfirm?: () => unknown }) => {
		await o.onconfirm?.();
		return true;
	});
});

describe('TemplateAppStatus', () => {
	it('shows an update with its notes and upgrades after a confirmation', async () => {
		const user = userEvent.setup();
		api.getAppTemplate.mockResolvedValue(status());
		api.upgradeAppTemplate.mockResolvedValue({ deployment_id: 'dep_9' });
		render(TemplateAppStatusView, { props: { app } });
		expect(await screen.findByText('Update available: 3.4 → 3.5')).toBeInTheDocument();
		expect(screen.getByText(/migrates the database/)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Upgrade to 3.5' }));
		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({
				title: 'Upgrade stats to Umami 3.5?',
				details: expect.arrayContaining([expect.stringMatching(/snapshot/i)])
			})
		);
		expect(api.upgradeAppTemplate).toHaveBeenCalledWith('app_1');
		expect(nav.goto).toHaveBeenCalledWith('/apps/app_1/deployments/dep_9');
	});

	it('explains why an edited compose file blocks the upgrade', async () => {
		api.getAppTemplate.mockResolvedValue(
			status({ upgrade_blocked: 'The compose file was edited after it was deployed.' })
		);
		render(TemplateAppStatusView, { props: { app } });
		expect(await screen.findByText(/was edited after/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /upgrade to/i })).not.toBeInTheDocument();
	});

	it('offers to update values still on the old address', async () => {
		const user = userEvent.setup();
		const change = {
			key: 'APP_URL',
			current: 'https://stats.apps.example.com',
			proposed: 'https://stats.example.org'
		};
		api.getAppTemplate.mockResolvedValue(status({ update_available: false, domain_env: [change] }));
		api.syncTemplateDomainEnv.mockResolvedValue({ updated: [change], deployment_id: 'dep_2' });
		render(TemplateAppStatusView, { props: { app } });
		expect(await screen.findByText(/APP_URL still uses/)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /update and redeploy/i }));
		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({ details: domainEnvDetails([change]) })
		);
		expect(api.syncTemplateDomainEnv).toHaveBeenCalledWith('app_1', { redeploy: true });
	});
});
