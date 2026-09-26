import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DatabaseBackupStatus, ManagedDatabaseDetail, Snapshot } from '$lib/api';

const api = vi.hoisted(() => ({
	getDatabaseBackupSchedule: vi.fn(),
	updateDatabaseBackupSchedule: vi.fn(),
	restoreDatabaseToNew: vi.fn(),
	listDatabases: vi.fn(),
	updateDatabase: vi.fn(),
	getDatabase: vi.fn(),
	confirmDatabaseUpgrade: vi.fn(),
	rollbackDatabaseUpgrade: vi.fn(),
	dismissDatabaseUpgrade: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

import BackupSchedule from './database-backup-schedule.svelte';
import RestoreToNewDialog from './restore-to-new-dialog.svelte';
import PublicSecurity from './database-public-security.svelte';
import UpgradeBanner from './database-upgrade-banner.svelte';

function status(overrides: Partial<DatabaseBackupStatus> = {}): DatabaseBackupStatus {
	return {
		schedule: null,
		retention: 7,
		next_run_at: null,
		full_snapshots_enabled: false,
		full_schedule: null,
		last_backup_at: null,
		last_status: null,
		last_error: null,
		warning: 'Nothing backs up shop automatically.',
		...overrides
	};
}

function db(overrides: Partial<ManagedDatabaseDetail> = {}): ManagedDatabaseDetail {
	return {
		id: 'db1',
		name: 'shop',
		db_type: 'postgres',
		project_id: null,
		host_port: 5401,
		status: 'running',
		db_name: 'tond_shop',
		db_user: 'tond_shop',
		has_password: true,
		has_root_password: false,
		memory_limit: null,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'postgres:16-alpine',
		custom_image: null,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		public_access: true,
		version: '16',
		variant: '',
		backup_schedule: null,
		backup_retention: 7,
		allowlist: [],
		tls: false,
		upgrade: null,
		connection_string: null,
		public_connection_string: null,
		internal_connection_string: null,
		upgrade_targets: ['17'],
		access_warning: 'Open to every address on the internet.',
		allowlist_enforced: false,
		supports_tls: true,
		backup_warning: null,
		...overrides
	};
}

beforeEach(() => {
	for (const fn of Object.values(api)) fn.mockReset();
	document.body.style.pointerEvents = '';
});

describe('DatabaseBackupSchedule', () => {
	it('warns when nothing backs the database up', async () => {
		api.getDatabaseBackupSchedule.mockResolvedValue(status());
		render(BackupSchedule, { props: { dbId: 'db1', dbName: 'shop', canEdit: true } });
		expect(await screen.findByText(/Nothing backs up shop/)).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /Save schedule/ })).toBeDisabled();
	});

	it('saves an edited custom schedule and retention', async () => {
		const user = userEvent.setup();
		api.getDatabaseBackupSchedule.mockResolvedValue(
			status({ schedule: '15 4 * * *', warning: null, next_run_at: '2026-09-27T04:15:00Z' })
		);
		api.updateDatabaseBackupSchedule.mockResolvedValue(
			status({ schedule: '30 5 * * *', retention: 3, warning: null })
		);
		render(BackupSchedule, { props: { dbId: 'db1', dbName: 'shop', canEdit: true } });
		const cron = await screen.findByLabelText('Cron expression (UTC)');
		expect(cron).toHaveValue('15 4 * * *');
		await user.clear(cron);
		await user.type(cron, '30 5 * * *');
		expect(screen.getByText(/05:30 \(UTC\)/)).toBeInTheDocument();
		const keep = screen.getByLabelText('Keep');
		await user.clear(keep);
		await user.type(keep, '3');
		await user.click(screen.getByRole('button', { name: /Save schedule/ }));
		await waitFor(() =>
			expect(api.updateDatabaseBackupSchedule).toHaveBeenCalledWith('db1', {
				schedule: '30 5 * * *',
				retention: 3
			})
		);
		expect(await screen.findByText(/Backups of shop scheduled/)).toBeInTheDocument();
	});

	it('refuses to keep zero backups and shows a failed latest backup', async () => {
		const user = userEvent.setup();
		api.getDatabaseBackupSchedule.mockResolvedValue(
			status({
				schedule: '0 3 * * *',
				warning: null,
				last_status: 'failed',
				last_error: 'pg_dump exited with code 1'
			})
		);
		render(BackupSchedule, { props: { dbId: 'db1', dbName: 'shop', canEdit: true } });
		expect(await screen.findByText('pg_dump exited with code 1')).toBeInTheDocument();
		const keep = screen.getByLabelText('Keep');
		await user.clear(keep);
		await user.type(keep, '0');
		expect(screen.getByText('Keep between 1 and 365.')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /Save schedule/ })).toBeDisabled();
	});
});

describe('RestoreToNewDialog', () => {
	const snap = { id: 's1', created_at: '2026-09-25T10:00:00Z' } as Snapshot;

	it('suggests a free name, checks it, and restores into the new database', async () => {
		const user = userEvent.setup();
		api.listDatabases.mockResolvedValue([{ name: 'shop' }, { name: 'shop-restore-20260925' }]);
		const onrestored = vi.fn();
		api.restoreDatabaseToNew.mockResolvedValue({
			message: 'Restored',
			database: { id: 'db2', name: 'shop-restore-20260925-2' }
		});
		render(RestoreToNewDialog, {
			props: { open: true, snapshot: snap, source: 'shop', onrestored }
		});
		const input = await screen.findByLabelText('Name of the new database');
		await waitFor(() => expect(input).toHaveValue('shop-restore-20260925-2'));

		await user.clear(input);
		await user.type(input, 'shop');
		expect(screen.getByText('A database with this name exists')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Create and restore' })).toBeDisabled();

		await user.clear(input);
		await user.type(input, 'shop-check');
		await user.click(screen.getByRole('button', { name: 'Create and restore' }));
		await waitFor(() =>
			expect(api.restoreDatabaseToNew).toHaveBeenCalledWith('s1', {
				source: 'shop',
				name: 'shop-check'
			})
		);
		expect(onrestored).toHaveBeenCalled();
	});

	it('keeps the dialog open with the server’s reason when the restore fails', async () => {
		const user = userEvent.setup();
		api.listDatabases.mockResolvedValue([]);
		api.restoreDatabaseToNew.mockRejectedValue(new Error('loading the dump failed'));
		render(RestoreToNewDialog, { props: { open: true, snapshot: snap, source: 'shop' } });
		await user.click(await screen.findByRole('button', { name: 'Create and restore' }));
		expect(await screen.findByText('loading the dump failed')).toBeInTheDocument();
	});
});

describe('DatabasePublicSecurity', () => {
	it('shows the open-to-everyone warning and saves a valid allowlist', async () => {
		const user = userEvent.setup();
		const onchange = vi.fn();
		api.updateDatabase.mockResolvedValue(
			db({ allowlist: ['203.0.113.7'], access_warning: null, allowlist_enforced: true })
		);
		render(PublicSecurity, { props: { db: db(), canEdit: true, onchange } });
		expect(screen.getByText(/Open to every address/)).toBeInTheDocument();

		const area = screen.getByLabelText('Allowed addresses');
		await user.type(area, '2001:db8::1');
		expect(screen.getByText(/isn't an IPv4 address/)).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Save allowed addresses' })).toBeDisabled();

		await user.clear(area);
		await user.type(area, '203.0.113.7');
		await user.click(screen.getByRole('button', { name: 'Save allowed addresses' }));
		await waitFor(() =>
			expect(api.updateDatabase).toHaveBeenCalledWith('db1', { allowlist: ['203.0.113.7'] })
		);
		expect(onchange).toHaveBeenCalled();
	});

	it('is read-only for non-admins and hides TLS for engines without it', () => {
		render(PublicSecurity, {
			props: { db: db({ supports_tls: false }), canEdit: false, onchange: () => {} }
		});
		expect(screen.getByLabelText('Allowed addresses')).toBeDisabled();
		expect(screen.queryByRole('button', { name: /TLS/ })).not.toBeInTheDocument();
	});
});

describe('DatabaseUpgradeBanner', () => {
	const upgrade = {
		from_version: '16',
		to_version: '17',
		from_image: 'postgres:16-alpine',
		to_image: 'postgres:17-alpine',
		step: 'Copying the data into version 17',
		backup_id: 'b1',
		started_at: '2026-01-01T00:00:00Z',
		finished_at: null,
		error: null
	};

	it('renders nothing without an upgrade', () => {
		const { container } = render(UpgradeBanner, {
			props: { db: db(), canManage: true, onchange: () => {} }
		});
		expect(container.querySelector('[data-testid="database-upgrade"]')).toBeNull();
	});

	it('shows progress while running', () => {
		render(UpgradeBanner, {
			props: {
				db: db({ upgrade: { ...upgrade, status: 'running' } }),
				canManage: true,
				onchange: () => {}
			}
		});
		expect(screen.getByText(/Upgrading shop from version 16 to 17/)).toBeInTheDocument();
		expect(screen.getByText(/Copying the data/)).toBeInTheDocument();
	});

	it('offers keep and roll back to admins once the new version runs', () => {
		render(UpgradeBanner, {
			props: {
				db: db({ upgrade: { ...upgrade, status: 'awaiting_confirmation' } }),
				canManage: true,
				onchange: () => {}
			}
		});
		expect(screen.getByRole('button', { name: 'Keep version 17' })).toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Roll back to 16' })).toBeInTheDocument();
	});

	it('explains a failed upgrade and can dismiss it', async () => {
		const user = userEvent.setup();
		api.dismissDatabaseUpgrade.mockResolvedValue(undefined);
		api.getDatabase.mockResolvedValue(db());
		const onchange = vi.fn();
		render(UpgradeBanner, {
			props: {
				db: db({
					upgrade: { ...upgrade, status: 'failed', error: 'the new version didn’t start' }
				}),
				canManage: false,
				onchange
			}
		});
		expect(screen.getByText(/the new version didn’t start/)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Dismiss' }));
		await waitFor(() => expect(onchange).toHaveBeenCalled());
	});
});
