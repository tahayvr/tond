import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listApps } = vi.hoisted(() => ({
	listApps: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listApps,
	ALERT_GROUPS: [{ id: 'resource', label: 'Resource', metrics: [], events: [] }]
}));

describe('alerts/new page load', () => {
	beforeEach(() => {
		listApps.mockReset();
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
	});

	it('loads alert creation route data before render', async () => {
		const pageModule = await import('../../routes/alerts/new/+page');

		await expect(pageModule.load()).resolves.toEqual({
			apps: [{ id: 'app_1', name: 'api-service' }]
		});
	});
});
