import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getCronJob, listProjects } = vi.hoisted(() => ({
	getCronJob: vi.fn(),
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getCronJob,
	listProjects
}));

describe('cron/[id] page load', () => {
	beforeEach(() => {
		getCronJob.mockReset();
		listProjects.mockReset();
		getCronJob.mockResolvedValue({ id: 'cron_1', name: 'nightly' });
		listProjects.mockResolvedValue([{ id: 'proj_1', name: 'Platform' }]);
	});

	it('loads cron detail route data before render', async () => {
		const pageModule = await import('../../routes/cron/[id]/+page');

		await expect(pageModule.load({ params: { id: 'cron_1' } } as never)).resolves.toEqual({
			job: { id: 'cron_1', name: 'nightly' },
			projects: [{ id: 'proj_1', name: 'Platform' }]
		});
	});
});
