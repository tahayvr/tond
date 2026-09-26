import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listAlertRules, listActiveAlerts, listApps } = vi.hoisted(() => ({
	listAlertRules: vi.fn(),
	listActiveAlerts: vi.fn(),
	listApps: vi.fn()
}));

vi.mock('$lib/api', () => ({ listAlertRules, listActiveAlerts, listApps }));

describe('alerts page load', () => {
	beforeEach(() => {
		listAlertRules.mockReset().mockResolvedValue([{ id: 'alert_1', name: 'CPU high' }]);
		listActiveAlerts.mockReset().mockResolvedValue([{ rule_id: 'alert_1', state: 'firing' }]);
		listApps.mockReset().mockResolvedValue([{ id: 'app_1', name: 'api' }]);
	});

	it('loads rules, live alerts and apps before render', async () => {
		const pageModule = await import('../../routes/alerts/+page');
		await expect(pageModule.load()).resolves.toEqual({
			rules: [{ id: 'alert_1', name: 'CPU high' }],
			active: [{ rule_id: 'alert_1', state: 'firing' }],
			apps: [{ id: 'app_1', name: 'api' }]
		});
	});

	it('still loads when the apps list is not allowed', async () => {
		listApps.mockRejectedValue(new Error('forbidden'));
		const pageModule = await import('../../routes/alerts/+page');
		await expect(pageModule.load()).resolves.toMatchObject({ apps: [] });
	});
});
