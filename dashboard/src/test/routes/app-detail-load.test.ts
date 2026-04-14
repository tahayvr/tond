import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getApp } = vi.hoisted(() => ({
	getApp: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getApp
}));

describe('apps/[id] page load', () => {
	beforeEach(() => {
		getApp.mockReset();
		getApp.mockResolvedValue({ id: 'app_1', name: 'api-service', status: 'running' });
	});

	it('loads the app detail before rendering the page', async () => {
		const pageModule = await import('../../routes/apps/[id]/+page');

		const data = await pageModule.load({ params: { id: 'app_1' } } as never);

		expect(getApp).toHaveBeenCalledWith('app_1');
		expect(data).toEqual({
			app: { id: 'app_1', name: 'api-service', status: 'running' }
		});
	});

	it('throws when the app detail cannot be loaded', async () => {
		const pageModule = await import('../../routes/apps/[id]/+page');
		getApp.mockRejectedValue(new Error('App not found'));

		await expect(pageModule.load({ params: { id: 'missing' } } as never)).rejects.toThrow(
			'App not found'
		);
	});
});
