import { beforeEach, describe, expect, it, vi } from 'vitest';

const { listAlertRules } = vi.hoisted(() => ({
	listAlertRules: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listAlertRules
}));

describe('alerts page load', () => {
	beforeEach(() => {
		listAlertRules.mockReset();
		listAlertRules.mockResolvedValue([{ id: 'alert_1', name: 'CPU high' }]);
	});

	it('loads alert rules route data before render', async () => {
		const pageModule = await import('../../routes/alerts/+page');

		await expect(pageModule.load()).resolves.toEqual({
			rules: [{ id: 'alert_1', name: 'CPU high' }]
		});
	});
});
