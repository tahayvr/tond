import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { RemoteSnapshot } from '$lib/api';

const api = vi.hoisted(() => ({
	listStorageProviders: vi.fn(),
	getBackupConfig: vi.fn(),
	listApps: vi.fn(),
	listDatabases: vi.fn(),
	listRemoteSnapshots: vi.fn(),
	importRemoteSnapshot: vi.fn(),
	restoreSnapshot: vi.fn(),
	rollbackAppToSnapshot: vi.fn(),
	restoreDatabase: vi.fn(),
	restoreDatabaseToNew: vi.fn()
}));

vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	...api
}));
vi.mock('$app/navigation', () => ({ goto: vi.fn(), invalidateAll: vi.fn() }));

import { ApiError } from '$lib/api';
import RemoteSnapshots from './remote-snapshots.svelte';
import { confirmDialog, renderConfirmHost } from '../../../test/confirm';

const full: RemoteSnapshot = {
	id: 'snap_full',
	provider_id: 'sp_1',
	created_at: '2026-10-20T03:00:00.000Z',
	tond_version: '1.0.0',
	archive_bytes: 5 * 1024 * 1024,
	readable: true,
	name: 'Scheduled nightly',
	trigger: 'scheduled',
	scope: 'full',
	apps: ['web', 'gone-app'],
	databases: ['shop', 'old-db'],
	imported: false
};

const otherKey: RemoteSnapshot = {
	...full,
	id: 'snap_other',
	readable: false,
	name: null,
	trigger: null,
	scope: null,
	apps: [],
	databases: []
};

function setup(remote: RemoteSnapshot[] = [full, otherKey]) {
	api.listStorageProviders.mockResolvedValue([
		{ id: 'sp_1', name: 'R2', kind: 's3', config: { bucket: 'tond-backups' } },
		{ id: 'sp_2', name: 'S3', kind: 's3', config: { bucket: 'dr' } }
	]);
	api.getBackupConfig.mockResolvedValue({ storage_provider_id: 'sp_1' });
	api.listApps.mockResolvedValue([{ name: 'web' }]);
	api.listDatabases.mockResolvedValue([{ name: 'shop' }]);
	api.listRemoteSnapshots.mockResolvedValue(remote);
	api.importRemoteSnapshot.mockResolvedValue({ id: 'snap_full' });
	renderConfirmHost();
	render(RemoteSnapshots);
}

describe('restore from storage', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
	});

	it('lists the configured provider and explains snapshots made with another key', async () => {
		setup();
		expect(await screen.findByText('Scheduled nightly')).toBeInTheDocument();
		expect(api.listRemoteSnapshots).toHaveBeenCalledWith('sp_1');
		const rows = screen.getAllByTestId('remote-snapshot');
		expect(within(rows[1]).getByText(/Made with a different key/)).toBeInTheDocument();
		expect(within(rows[1]).queryByRole('button', { name: /Restore/ })).toBeNull();
	});

	it('restores the whole server after importing, with typed confirmation', async () => {
		const user = userEvent.setup();
		api.restoreSnapshot.mockResolvedValue({ pre_restore_snapshot_id: 'p', message: 'ok' });
		setup();
		await user.click(await screen.findByRole('button', { name: /Restore…/ }));
		// Apps missing here are named, not offered.
		expect(screen.getByText(/Not on this server: gone-app/)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /Review restore/ }));
		const dialog = await screen.findByRole('alertdialog');
		expect(within(dialog).getByText(/redeploying those whose containers are missing/)).toBeTruthy();
		await confirmDialog(user, { typed: 'restore' });
		expect(api.importRemoteSnapshot).toHaveBeenCalledWith('snap_full', 'sp_1');
		expect(api.restoreSnapshot).toHaveBeenCalledWith('snap_full');
		expect(await screen.findByText(/Full restore started/)).toBeInTheDocument();
	});

	it('restores one app', async () => {
		const user = userEvent.setup();
		api.rollbackAppToSnapshot.mockResolvedValue({ message: 'web rolled back.' });
		setup();
		await user.click(await screen.findByRole('button', { name: /Restore…/ }));
		await user.click(screen.getByLabelText(/App web/));
		await user.click(screen.getByRole('button', { name: /Review restore/ }));
		await confirmDialog(user, { typed: 'web' });
		expect(api.rollbackAppToSnapshot).toHaveBeenCalledWith('snap_full', 'web');
		expect(await screen.findByText('web rolled back.')).toBeInTheDocument();
	});

	it('offers databases missing here as a restore into a new database', async () => {
		const user = userEvent.setup();
		api.restoreDatabaseToNew.mockResolvedValue({ message: 'Restored old-db.' });
		setup();
		await user.click(await screen.findByRole('button', { name: /Restore…/ }));
		await user.click(screen.getByLabelText(/Not on this server: restored into a new database/));
		const name = screen.getByLabelText('New database name');
		await user.clear(name);
		await user.type(name, 'old-db-copy');
		await user.click(screen.getByRole('button', { name: /Review restore/ }));
		await confirmDialog(user);
		expect(api.restoreDatabaseToNew).toHaveBeenCalledWith('snap_full', {
			source: 'old-db',
			name: 'old-db-copy'
		});
	});

	it('shows the key mismatch hint inside the dialog', async () => {
		const user = userEvent.setup();
		setup();
		api.importRemoteSnapshot.mockRejectedValue(
			new ApiError(
				'Encrypted with a different TOND_SECRET_KEY.',
				409,
				null,
				'SNAPSHOT_KEY_MISMATCH',
				{
					hint: 'Set TOND_SECRET_KEY to the old key and restart.'
				}
			)
		);
		await user.click(await screen.findByRole('button', { name: /Restore…/ }));
		await user.click(screen.getByLabelText(/Database shop/));
		await user.click(screen.getByRole('button', { name: /Review restore/ }));
		await confirmDialog(user, { typed: 'shop' });
		expect(await screen.findByText(/old key and restart/)).toBeInTheDocument();
		expect(api.restoreDatabase).not.toHaveBeenCalled();
	});

	it('links to provider setup when none exists', async () => {
		api.listStorageProviders.mockResolvedValue([]);
		api.getBackupConfig.mockResolvedValue({ storage_provider_id: null });
		api.listApps.mockResolvedValue([]);
		api.listDatabases.mockResolvedValue([]);
		render(RemoteSnapshots);
		const link = await screen.findByRole('link', { name: 'Add a storage provider' });
		expect(link).toHaveAttribute('href', '/integrations');
		expect(api.listRemoteSnapshots).not.toHaveBeenCalled();
	});
});
