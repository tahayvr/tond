import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listProjects, listApps, listDatabases, listCronJobs, listAuditLog, getSettings } =
	vi.hoisted(() => ({
		listProjects: vi.fn(),
		listApps: vi.fn(),
		listDatabases: vi.fn(),
		listCronJobs: vi.fn(),
		listAuditLog: vi.fn(),
		getSettings: vi.fn()
	}));

vi.mock('$lib/api', () => ({
	listProjects,
	listApps,
	listDatabases,
	listCronJobs,
	listAuditLog,
	getSettings
}));

describe('dashboard home page load', () => {
	beforeEach(() => {
		listProjects.mockReset();
		listApps.mockReset();
		listDatabases.mockReset();
		listCronJobs.mockReset();
		listAuditLog.mockReset();
		getSettings.mockReset();

		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
		listApps.mockResolvedValue([{ id: 'app_1', name: 'api-service', status: 'running' }]);
		listDatabases.mockResolvedValue([{ id: 'db_1', name: 'postgres', status: 'running' }]);
		listCronJobs.mockResolvedValue([{ id: 'cron_1', name: 'hourly-report', enabled: true }]);
		listAuditLog.mockResolvedValue([{ id: 'log_1', action: 'deploy' }]);
		getSettings.mockResolvedValue({ server_name: 'Tond Server' });
	});

	it('loads dashboard route data before the page renders', async () => {
		const pageModule = await import('../../routes/+page');

		const data = await pageModule.load();

		expect(data).toEqual({
			projects: [{ id: 'proj_1', name: 'Platform' }],
			apps: [{ id: 'app_1', name: 'api-service', status: 'running' }],
			databases: [{ id: 'db_1', name: 'postgres', status: 'running' }],
			cronJobs: [{ id: 'cron_1', name: 'hourly-report', enabled: true }],
			recentActivity: [{ id: 'log_1', action: 'deploy' }],
			serverName: 'Tond Server'
		});
		expect(listAuditLog).toHaveBeenCalledWith({ limit: 5 });
	});

	it('throws when dashboard route data fails to load', async () => {
		const pageModule = await import('../../routes/+page');
		listProjects.mockRejectedValue(new Error('Dashboard API unavailable'));

		await expect(pageModule.load()).rejects.toThrow('Dashboard API unavailable');
	});
});
