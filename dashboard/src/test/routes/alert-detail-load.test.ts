import { beforeEach, describe, expect, it, vi } from 'vitest';

const api = vi.hoisted(() => ({
	getAlertRule: vi.fn(),
	listActiveAlerts: vi.fn(),
	listAlertKinds: vi.fn(),
	listApps: vi.fn(),
	listIntegrations: vi.fn(),
	getAlertSettings: vi.fn()
}));

vi.mock('$lib/api', () => api);

describe('alerts/[id] page load', () => {
	beforeEach(() => {
		api.getAlertRule.mockReset().mockResolvedValue({ id: 'alert_1', name: 'CPU high' });
		api.listActiveAlerts.mockReset().mockResolvedValue([
			{ rule_id: 'alert_1', target: 'app:a' },
			{ rule_id: 'alert_2', target: 'host' }
		]);
		api.listAlertKinds.mockReset().mockResolvedValue([{ kind: 'cpu' }]);
		api.listApps.mockReset().mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
		// A viewer can't list channels: the page still loads.
		api.listIntegrations.mockReset().mockRejectedValue(new Error('forbidden'));
		api.getAlertSettings.mockReset().mockResolvedValue({ quiet_hours: { enabled: true } });
	});

	it('loads the rule with its own live alerts', async () => {
		const pageModule = await import('../../routes/alerts/[id]/+page');

		await expect(pageModule.load({ params: { id: 'alert_1' } } as never)).resolves.toEqual({
			rule: { id: 'alert_1', name: 'CPU high' },
			active: [{ rule_id: 'alert_1', target: 'app:a' }],
			kinds: [{ kind: 'cpu' }],
			apps: [{ id: 'app_1', name: 'api-service' }],
			integrations: [],
			quietHours: { enabled: true }
		});
	});
});
