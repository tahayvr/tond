import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getAlertRule, listApps } = vi.hoisted(() => ({
	getAlertRule: vi.fn(),
	listApps: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getAlertRule,
	listApps,
	ALERT_GROUPS: [{ id: 'resource', label: 'Resource', metrics: [], events: [] }]
}));

describe('alerts/[id] page load', () => {
	beforeEach(() => {
		getAlertRule.mockReset();
		listApps.mockReset();
		getAlertRule.mockResolvedValue({ id: 'alert_1', name: 'CPU high', metric: null, events: [] });
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
	});

	it('loads alert detail route data before render', async () => {
		const pageModule = await import('../../routes/alerts/[id]/+page');

		await expect(pageModule.load({ params: { id: 'alert_1' } } as never)).resolves.toEqual({
			rule: { id: 'alert_1', name: 'CPU high', metric: null, events: [] },
			apps: [{ id: 'app_1', name: 'api-service' }]
		});
	});
});
