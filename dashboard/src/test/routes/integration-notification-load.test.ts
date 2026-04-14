import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listIntegrations } = vi.hoisted(() => ({
	listIntegrations: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listIntegrations
}));

describe('notification integration pages load', () => {
	beforeEach(() => {
		listIntegrations.mockReset();
		listIntegrations.mockResolvedValue([
			{ id: 'int_1', kind: 'email', name: 'Email Alerts' },
			{ id: 'int_2', kind: 'slack', name: 'Slack Alerts' },
			{ id: 'int_3', kind: 'telegram', name: 'Telegram Alerts' },
			{ id: 'int_4', kind: 'discord', name: 'Discord Alerts' }
		]);
	});

	it('loads email integrations before render', async () => {
		const pageModule = await import('../../routes/integrations/email/+page');

		await expect(pageModule.load()).resolves.toEqual({
			integrations: [{ id: 'int_1', kind: 'email', name: 'Email Alerts' }]
		});
	});

	it('loads slack integrations before render', async () => {
		const pageModule = await import('../../routes/integrations/slack/+page');

		await expect(pageModule.load()).resolves.toEqual({
			integrations: [{ id: 'int_2', kind: 'slack', name: 'Slack Alerts' }]
		});
	});

	it('loads telegram integrations before render', async () => {
		const pageModule = await import('../../routes/integrations/telegram/+page');

		await expect(pageModule.load()).resolves.toEqual({
			integrations: [{ id: 'int_3', kind: 'telegram', name: 'Telegram Alerts' }]
		});
	});

	it('loads discord integrations before render', async () => {
		const pageModule = await import('../../routes/integrations/discord/+page');

		await expect(pageModule.load()).resolves.toEqual({
			integrations: [{ id: 'int_4', kind: 'discord', name: 'Discord Alerts' }]
		});
	});
});
