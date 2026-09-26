import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listApps, listIntegrations } = vi.hoisted(() => ({
	listApps: vi.fn(),
	listIntegrations: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listApps,
	listIntegrations,
	ALERT_GROUPS: [{ id: 'resource', label: 'Resource', metrics: [], events: [] }]
}));

describe('alerts/new page load', () => {
	beforeEach(() => {
		listApps.mockReset();
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
		listIntegrations.mockReset();
		listIntegrations.mockResolvedValue([{ id: 'int_1', name: 'ops', kind: 'slack' }]);
	});

	it('loads alert creation route data before render', async () => {
		const pageModule = await import('../../routes/alerts/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			apps: [{ id: 'app_1', name: 'api-service' }],
			integrations: [{ id: 'int_1', name: 'ops', kind: 'slack' }]
		});
	});
});
