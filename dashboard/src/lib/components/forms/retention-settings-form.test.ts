import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RetentionSettings } from '$lib/api';
import RetentionSettingsForm from './retention-settings-form.svelte';

const api = vi.hoisted(() => ({
	getRetention: vi.fn(),
	updateRetention: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

function settings(auditDays = 180): RetentionSettings {
	return {
		fields: [
			{
				key: 'retention_audit_log_days',
				value: auditDays,
				default: 180,
				min: 30,
				max: 3650,
				unit: 'days'
			}
		],
		database: {
			file_bytes: 10 * 1024 * 1024,
			wal_bytes: 0,
			free_bytes: 0,
			tables: [{ name: 'audit_log', bytes: 1024 * 1024 }],
			measured_at: new Date().toISOString()
		}
	};
}

describe('RetentionSettingsForm', () => {
	beforeEach(() => {
		api.getRetention.mockReset().mockResolvedValue(settings());
		api.updateRetention.mockReset().mockResolvedValue(settings(365));
	});

	it('shows the database size and saves only what changed', async () => {
		render(RetentionSettingsForm, { canEdit: true });
		const input = await screen.findByLabelText('Activity (audit) log for');
		expect(screen.getByText('10.0 MB')).toBeTruthy();
		expect(screen.getByText('Activity log')).toBeTruthy();

		const save = screen.getByRole('button', { name: 'Save retention' });
		expect((save as HTMLButtonElement).disabled).toBe(true);

		await userEvent.clear(input);
		await userEvent.type(input, '365');
		await userEvent.click(save);
		await waitFor(() =>
			expect(api.updateRetention).toHaveBeenCalledWith({ retention_audit_log_days: 365 })
		);
		expect(await screen.findByText(/Saved\./)).toBeTruthy();
	});

	it('blocks out-of-range values', async () => {
		render(RetentionSettingsForm, { canEdit: true });
		const input = await screen.findByLabelText('Activity (audit) log for');
		await userEvent.clear(input);
		await userEvent.type(input, '5');
		expect(screen.getByText('Between 30 and 3650 days.')).toBeTruthy();
		expect(
			(screen.getByRole('button', { name: 'Save retention' }) as HTMLButtonElement).disabled
		).toBe(true);
	});

	it('is read-only without settings:write', async () => {
		render(RetentionSettingsForm, { canEdit: false });
		const input = await screen.findByLabelText('Activity (audit) log for');
		expect((input as HTMLInputElement).disabled).toBe(true);
		expect(screen.queryByRole('button', { name: 'Save retention' })).toBeNull();
	});
});
