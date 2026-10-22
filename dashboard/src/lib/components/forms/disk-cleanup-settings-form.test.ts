import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ServerSettings } from '$lib/api';
import DiskCleanupSettingsForm from './disk-cleanup-settings-form.svelte';

const api = vi.hoisted(() => ({ updateSettings: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const settings: ServerSettings = {
	server_name: 'tond',
	dashboard_domain: '',
	preview_domain: '',
	apps_domain: '',
	upload_max_bytes: 1,
	auto_prune_disk_percent: 85
};

describe('DiskCleanupSettingsForm', () => {
	beforeEach(() => {
		api.updateSettings.mockReset();
		api.updateSettings.mockImplementation(async (s: Partial<ServerSettings>) => ({
			...settings,
			...s
		}));
	});

	it('saves a new threshold and validates its range', async () => {
		const user = userEvent.setup();
		render(DiskCleanupSettingsForm, { props: { settings, canEdit: true } });
		const input = screen.getByLabelText(/when the disk is at least/i);
		expect(input).toHaveValue(85);
		await user.clear(input);
		await user.type(input, '40');
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(await screen.findByText(/between 50 and 99 percent/)).toBeInTheDocument();
		expect(api.updateSettings).not.toHaveBeenCalled();

		await user.clear(input);
		await user.type(input, '90');
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(api.updateSettings).toHaveBeenCalledWith({ auto_prune_disk_percent: 90 });
		expect(await screen.findByText('Saved.')).toBeInTheDocument();
	});

	it('turns automatic cleanup off with 0', async () => {
		const user = userEvent.setup();
		render(DiskCleanupSettingsForm, { props: { settings, canEdit: true } });
		await user.click(screen.getByRole('checkbox', { name: /clean up automatically/i }));
		expect(screen.getByLabelText(/when the disk is at least/i)).toBeDisabled();
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(api.updateSettings).toHaveBeenCalledWith({ auto_prune_disk_percent: 0 });
	});
});
