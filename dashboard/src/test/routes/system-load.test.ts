import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getSystemMetrics } = vi.hoisted(() => ({
	getSystemMetrics: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getSystemMetrics
}));

describe('system page load', () => {
	beforeEach(() => {
		getSystemMetrics.mockReset();
		getSystemMetrics.mockResolvedValue({
			cpu_percent: 20,
			mem_used: 512,
			mem_total: 2048,
			mem_percent: 25,
			disk_used: 10 * 1024 * 1024 * 1024,
			disk_total: 100 * 1024 * 1024 * 1024,
			disk_percent: 10,
			disk_mount: '/',
			containers_running: 3,
			apps_total: 5,
			apps_running: 4,
			collected_at: '2026-01-01T00:00:00.000Z'
		});
	});

	it('loads system metrics route data before render', async () => {
		const pageModule = await import('../../routes/system/+page');

		await expect(pageModule.load()).resolves.toEqual({
			metrics: {
				cpu_percent: 20,
				mem_used: 512,
				mem_total: 2048,
				mem_percent: 25,
				disk_used: 10 * 1024 * 1024 * 1024,
				disk_total: 100 * 1024 * 1024 * 1024,
				disk_percent: 10,
				disk_mount: '/',
				containers_running: 3,
				apps_total: 5,
				apps_running: 4,
				collected_at: '2026-01-01T00:00:00.000Z'
			}
		});
	});
});
