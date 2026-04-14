import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listAuditLog, listApps } = vi.hoisted(() => ({
	listAuditLog: vi.fn(),
	listApps: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listAuditLog,
	listApps
}));

describe('activity page load', () => {
	beforeEach(() => {
		listAuditLog.mockReset();
		listApps.mockReset();
		listAuditLog.mockResolvedValue([{ id: 'log_1', action: 'deploy' }]);
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service' }]);
	});

	it('loads default activity route data before render', async () => {
		const pageModule = await import('../../routes/activity/+page');

		await expect(pageModule.load()).resolves.toEqual({
			entries: [{ id: 'log_1', action: 'deploy' }],
			apps: [{ id: 'app_1', name: 'api-service' }],
			initialFilterAppId: '',
			initialFilterAction: '',
			initialFilterLimit: 50
		});
		expect(listAuditLog).toHaveBeenCalledWith({ app_id: undefined, limit: 50 });
	});
});
