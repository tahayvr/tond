import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listSnapshots } = vi.hoisted(() => ({
	listSnapshots: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listSnapshots
}));

describe('snapshots page load', () => {
	beforeEach(() => {
		listSnapshots.mockReset();
		listSnapshots.mockResolvedValue([{ id: 'snap_1', name: 'before-deploy' }]);
	});

	it('loads snapshots route data before render', async () => {
		const pageModule = await import('../../routes/snapshots/+page');

		await expect(pageModule.load()).resolves.toEqual({
			snapshots: [{ id: 'snap_1', name: 'before-deploy' }]
		});
	});
});
