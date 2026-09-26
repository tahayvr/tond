import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { confirmDialog, renderConfirmHost } from '../../../test/confirm';
import { ApiError, type UpdateOverview, type UpdateRun } from '$lib/api';
import UpdatesPanel from './UpdatesPanel.svelte';

const api = vi.hoisted(() => ({
	getUpdateOverview: vi.fn(),
	checkForUpdates: vi.fn(),
	setUpdateCheck: vi.fn(),
	startUpdate: vi.fn(),
	startRollback: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const now = Math.floor(Date.now() / 1000);

function overview(over: Partial<UpdateOverview> = {}): UpdateOverview {
	return {
		current_version: '0.3.0',
		schema_version: 60,
		latest: {
			version: '0.4.0',
			tag: 'v0.4.0',
			name: 'Tond 0.4.0',
			url: 'https://github.com/tahayvr/tond/releases/tag/v0.4.0',
			published_at: '2026-09-20T10:00:00Z',
			body: '## Highlights\n\n- Updates from the dashboard'
		},
		update_available: true,
		changelog: [
			{
				version: '0.4.0',
				tag: 'v0.4.0',
				name: 'Tond 0.4.0',
				url: 'https://github.com/tahayvr/tond/releases/tag/v0.4.0',
				published_at: '2026-09-20T10:00:00Z',
				body: '## Highlights\n\n- Updates from the dashboard'
			}
		],
		check: { enabled: true, checked_at: now - 3600, error: null },
		run: null,
		rollback: null,
		unsupported_reason: null,
		...over
	};
}

function run(over: Partial<UpdateRun> = {}): UpdateRun {
	return {
		id: 'r1',
		kind: 'update',
		state: 'running',
		effective_state: 'running',
		step: 'snapshot',
		from_version: '0.3.0',
		to_version: '0.4.0',
		actor: 'admin@example.com',
		snapshot_id: null,
		started_at: now,
		updated_at: now,
		finished_at: null,
		message: 'Taking a full snapshot',
		pid: 1,
		database_restored: false,
		log: ['[update] Update 0.3.0 → 0.4.0 requested by admin@example.com'],
		...over
	};
}

describe('UpdatesPanel', () => {
	beforeEach(() => vi.clearAllMocks());

	it('shows the running version, the available release and its notes', async () => {
		api.getUpdateOverview.mockResolvedValue(overview());
		render(UpdatesPanel, { canManage: true });
		expect(await screen.findByText('Tond 0.3.0')).toBeTruthy();
		expect(screen.getByTestId('update-available').textContent).toContain('Tond 0.4.0 is available');
		expect(screen.getByText('Updates from the dashboard')).toBeTruthy();
		expect(screen.getByRole('button', { name: /update to 0.4.0/i })).toBeTruthy();
	});

	it('hides the controls from people who cannot change settings', async () => {
		api.getUpdateOverview.mockResolvedValue(overview());
		render(UpdatesPanel, { canManage: false });
		await screen.findByText('Tond 0.3.0');
		expect(screen.queryByRole('button', { name: /update to/i })).toBeNull();
		expect(screen.queryByRole('button', { name: /check now/i })).toBeNull();
		expect(screen.queryByLabelText(/check for new releases/i)).toBeNull();
	});

	it('says when Tond is up to date', async () => {
		api.getUpdateOverview.mockResolvedValue(
			overview({ current_version: '0.4.0', update_available: false, changelog: [] })
		);
		render(UpdatesPanel, { canManage: true });
		expect(await screen.findByText('Up to date')).toBeTruthy();
		expect(screen.queryByRole('button', { name: /update to/i })).toBeNull();
	});

	it('asks before updating, explains what happens, then shows the run', async () => {
		const user = userEvent.setup();
		api.getUpdateOverview.mockResolvedValue(overview());
		api.startUpdate.mockResolvedValue(run());
		renderConfirmHost();
		render(UpdatesPanel, { canManage: true });
		await user.click(await screen.findByRole('button', { name: /update to 0.4.0/i }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/full snapshot is taken first/i)).toBeTruthy();
		expect(within(dialog).getByText(/put back automatically/i)).toBeTruthy();
		await confirmDialog(user);
		expect(api.startUpdate).toHaveBeenCalledWith({ version: '0.4.0' });
		expect((await screen.findByTestId('run-title')).textContent).toBe(
			'Updating Tond 0.3.0 → 0.4.0'
		);
		expect(screen.getByRole('list', { name: 'Steps' })).toBeTruthy();
		expect(screen.getByRole('button', { name: /update running/i })).toHaveProperty(
			'disabled',
			true
		);
	});

	it('reports an automatic rollback with its reason and log', async () => {
		api.getUpdateOverview.mockResolvedValue(
			overview({
				run: run({
					state: 'rolled_back',
					effective_state: 'rolled_back',
					step: 'done',
					finished_at: now,
					database_restored: true,
					message: 'The update to 0.4.0 failed (tond-server 0.4.0 did not become healthy).',
					log: ['▶ Putting Tond 0.3.0 back']
				})
			})
		);
		render(UpdatesPanel, { canManage: true });
		expect((await screen.findByTestId('run-title')).textContent).toBe(
			'The update to 0.4.0 was rolled back'
		);
		expect(screen.getByText(/Nothing was lost/)).toBeTruthy();
		expect(screen.getByTestId('run-log').textContent).toContain('Putting Tond 0.3.0 back');
	});

	it('offers a rollback, warns about the database and asks first', async () => {
		const user = userEvent.setup();
		api.getUpdateOverview.mockResolvedValue(
			overview({
				current_version: '0.4.0',
				update_available: false,
				changelog: [],
				rollback: {
					to_version: '0.3.0',
					backup_created_at: now - 86_400,
					available_until: now + 6 * 86_400,
					restores_database: true
				}
			})
		);
		api.startRollback.mockResolvedValue(
			run({ kind: 'rollback', from_version: '0.4.0', to_version: '0.3.0' })
		);
		renderConfirmHost();
		render(UpdatesPanel, { canManage: true });
		const card = await screen.findByTestId('rollback-card');
		expect(within(card).getByText(/restores the database from before it/)).toBeTruthy();
		await user.click(within(card).getByRole('button', { name: /roll back/i }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/changed since then are lost/)).toBeTruthy();
		await confirmDialog(user);
		expect(api.startRollback).toHaveBeenCalled();
		expect((await screen.findByTestId('run-title')).textContent).toBe('Rolling back to Tond 0.3.0');
	});

	it('shows a restart instead of an error while the server is down mid-run', async () => {
		vi.useFakeTimers({ shouldAdvanceTime: true });
		try {
			api.getUpdateOverview
				.mockResolvedValueOnce(overview({ run: run({ step: 'migrate' }) }))
				.mockRejectedValue(new ApiError('Tond is not reachable', 502));
			render(UpdatesPanel, { canManage: true });
			await screen.findByTestId('run-title');
			await vi.advanceTimersByTimeAsync(2500);
			expect(await screen.findByText(/Tond is restarting/)).toBeTruthy();
			expect(screen.queryByRole('alert')).toBeNull();
		} finally {
			vi.useRealTimers();
		}
	});

	it('turns the daily check off', async () => {
		const user = userEvent.setup();
		api.getUpdateOverview.mockResolvedValue(overview());
		api.setUpdateCheck.mockResolvedValue(
			overview({ check: { enabled: false, checked_at: null, error: null } })
		);
		render(UpdatesPanel, { canManage: true });
		await user.click(await screen.findByLabelText(/check for new releases every day/i));
		expect(api.setUpdateCheck).toHaveBeenCalledWith(false);
	});
});
