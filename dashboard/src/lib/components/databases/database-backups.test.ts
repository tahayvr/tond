import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '$lib/api';

const api = vi.hoisted(() => ({
	listDatabaseBackups: vi.fn(),
	createDatabaseBackup: vi.fn(),
	restoreDatabase: vi.fn(),
	getDatabaseBackupSchedule: vi.fn(() => new Promise(() => {}))
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

import DatabaseBackups, { backupLocation, formatBytes } from './database-backups.svelte';

function snap(overrides: Partial<Snapshot>): Snapshot {
	return {
		id: 's1',
		name: 'shop 2026-09-25',
		status: 'completed',
		trigger: 'manual',
		scope: 'database:shop',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: '2026-09-25T10:00:00Z',
		completed_at: '2026-09-25T10:00:05Z',
		delta_bytes: 1024,
		total_bytes: 2_097_152,
		local_path: '/opt/tond/data/backups/snapshots/s1',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null,
		remote_status: null,
		remote_error: null,
		archive_bytes: null,
		...overrides
	};
}

describe('database backup helpers', () => {
	it('describes where a backup lives', () => {
		expect(backupLocation(snap({}))).toBe('On this server');
		expect(backupLocation(snap({ remote_status: 'uploaded' }))).toMatch(/remote storage/);
		expect(backupLocation(snap({ remote_status: 'failed' }))).toMatch(/failed/);
		expect(formatBytes(2_097_152)).toBe('2.0 MB');
	});
});

describe('DatabaseBackups', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		api.getDatabaseBackupSchedule.mockReturnValue(new Promise(() => {}));
		document.body.style.pointerEvents = '';
		api.listDatabaseBackups.mockResolvedValue([
			snap({}),
			snap({ id: 's2', scope: 'full', trigger: 'scheduled', created_at: '2026-09-24T03:00:00Z' })
		]);
	});

	it('lists own backups and full snapshots, and backs up on demand', async () => {
		api.createDatabaseBackup.mockResolvedValue(snap({ id: 's3', remote_status: 'uploading' }));
		const user = userEvent.setup();
		render(DatabaseBackups, { props: { dbId: 'db1', dbName: 'shop', canRestore: true } });

		expect(await screen.findByText('Database backup')).toBeInTheDocument();
		expect(screen.getByText('Full server snapshot')).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /back up now/i }));
		expect(api.createDatabaseBackup).toHaveBeenCalledWith('db1');
		expect(await screen.findByText(/uploading to remote storage/i)).toBeInTheDocument();
	});

	it('restores only after the database name is typed', async () => {
		api.restoreDatabase.mockResolvedValue({
			message: "Database 'shop' restored. The data it held just before is saved as a backup.",
			pre_restore_snapshot_id: 's9'
		});
		const user = userEvent.setup();
		render(DatabaseBackups, { props: { dbId: 'db1', dbName: 'shop', canRestore: true } });

		const restoreButtons = await screen.findAllByRole('button', { name: /restore…/i });
		await user.click(restoreButtons[0]);
		const dialog = await screen.findByRole('dialog');
		const confirm = within(dialog).getByRole('button', { name: /restore shop/i });
		expect(confirm).toBeDisabled();

		await user.type(within(dialog).getByLabelText(/type/i), 'sho');
		expect(confirm).toBeDisabled();
		await user.type(within(dialog).getByLabelText(/type/i), 'p');
		expect(confirm).toBeEnabled();
		await user.click(confirm);

		expect(api.restoreDatabase).toHaveBeenCalledWith('s1', 'shop');
		expect(await screen.findByText(/saved as a backup/i)).toBeInTheDocument();
	});

	it('hides restore from members', async () => {
		render(DatabaseBackups, { props: { dbId: 'db1', dbName: 'shop', canRestore: false } });
		expect(await screen.findByText(/can't restore databases/i)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /restore…/i })).not.toBeInTheDocument();
	});
});
