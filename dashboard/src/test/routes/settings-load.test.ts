import { beforeEach, describe, expect, it, vi } from 'vitest';

const { getSettings } = vi.hoisted(() => ({
	getSettings: vi.fn()
}));

vi.mock('$lib/api', () => ({
	getSettings
}));

describe('settings page load', () => {
	beforeEach(() => {
		getSettings.mockReset();
		getSettings.mockResolvedValue({ server_name: 'Tond', dashboard_domain: 'dash.example.com' });
	});

	it('loads settings route data before render', async () => {
		const pageModule = await import('../../routes/settings/+page');

		await expect(pageModule.load()).resolves.toEqual({
			settings: { server_name: 'Tond', dashboard_domain: 'dash.example.com' }
		});
	});
});
