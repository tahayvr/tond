import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listCronJobs } = vi.hoisted(() => ({
	listCronJobs: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listCronJobs
}));

describe('cron page load', () => {
	beforeEach(() => {
		listCronJobs.mockReset();
		listCronJobs.mockResolvedValue([{ id: 'cron_1', name: 'nightly' }]);
	});

	it('loads cron jobs route data before render', async () => {
		const pageModule = await import('../../routes/cron/+page');

		await expect(pageModule.load()).resolves.toEqual({
			jobs: [{ id: 'cron_1', name: 'nightly' }]
		});
	});
});
