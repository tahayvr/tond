import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listAuditLog } = vi.hoisted(() => ({ listAuditLog: vi.fn() }));

vi.mock('$lib/api', () => ({ listAuditLog }));

describe('activity page load', () => {
	beforeEach(() => {
		listAuditLog.mockReset();
		listAuditLog.mockResolvedValue({ entries: [{ seq: 3, action: 'deploy' }], next_before: 2 });
	});

	it('loads the first page with no filters', async () => {
		const pageModule = await import('../../routes/activity/+page');

		await expect(pageModule.load({ url: new URL('http://x/activity') })).resolves.toEqual({
			filters: { q: '', actor: '', action: '', resourceType: '', failuresOnly: false },
			entries: [{ seq: 3, action: 'deploy' }],
			nextBefore: 2
		});
		expect(listAuditLog).toHaveBeenCalledWith(
			expect.objectContaining({ limit: 50, actor: undefined })
		);
	});

	it('passes URL filters to the server instead of filtering after the limit', async () => {
		const pageModule = await import('../../routes/activity/+page');

		await pageModule.load({
			url: new URL('http://x/activity?actor=ci&action=login*&outcome=failure&resource_type=app')
		});
		expect(listAuditLog).toHaveBeenCalledWith(
			expect.objectContaining({
				actor: 'ci',
				action: 'login*',
				outcome: 'failure',
				resource_type: 'app',
				limit: 50
			})
		);
	});
});
