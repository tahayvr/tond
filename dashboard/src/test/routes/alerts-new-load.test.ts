import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listApps, listIntegrations, listAlertKinds, getAlertSettings } = vi.hoisted(() => ({
	listApps: vi.fn(),
	listIntegrations: vi.fn(),
	listAlertKinds: vi.fn(),
	getAlertSettings: vi.fn()
}));

vi.mock('$lib/api', () => ({ listApps, listIntegrations, listAlertKinds, getAlertSettings }));

describe('alerts/new page load', () => {
	beforeEach(() => {
		listApps.mockReset().mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
		listIntegrations.mockReset().mockResolvedValue([{ id: 'int_1', name: 'ops', kind: 'slack' }]);
		listAlertKinds.mockReset().mockResolvedValue([{ kind: 'cpu' }]);
		getAlertSettings.mockReset().mockResolvedValue({ quiet_hours: { enabled: false } });
	});

	it('loads kinds, apps, channels and quiet hours before render', async () => {
		const pageModule = await import('../../routes/alerts/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			kinds: [{ kind: 'cpu' }],
			apps: [{ id: 'app_1', name: 'api-service' }],
			integrations: [{ id: 'int_1', name: 'ops', kind: 'slack' }],
			quietHours: { enabled: false }
		});
	});
});
