import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Snapshot } from '$lib/api';

const { setUiContext, clearUiContext, snapshotsContext } = vi.hoisted(() => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	snapshotsContext: vi.fn(() => ({ type: 'snapshots' }))
}));

const api = vi.hoisted(() => ({
	createSnapshot: vi.fn(),
	deleteSnapshot: vi.fn(),
	restoreSnapshot: vi.fn(),
	rollbackAppToSnapshot: vi.fn(),
	listStorageProviders: vi.fn(),
	getBackupConfig: vi.fn(),
	updateBackupConfig: vi.fn()
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	snapshotsContext
}));

vi.mock('$lib/api', () => ({
	...api
}));

import SnapshotsPage from '../../routes/snapshots/+page.svelte';

const fullSnapshot: Snapshot = {
	id: 'snap_1',
	name: 'before-deploy',
	status: 'completed',
	scope: 'full',
	trigger: 'manual',
	trigger_app: null,
	deployment_id: null,
	parent_snapshot_id: null,
	completed_at: '2026-01-01T00:01:00.000Z',
	delta_bytes: 1024,
	total_bytes: 1024,
	local_path: null,
	remote_key: null,
	storage_provider_id: null,
	checksum_sha256: null,
	error_message: null,
	notes: null,
	created_at: '2026-01-01T00:00:00.000Z'
};

const preDeploySnapshot: Snapshot = {
	id: 'snap_2',
	name: 'pre-deploy-web',
	status: 'completed',
	scope: 'app',
	trigger: 'pre_deploy',
	trigger_app: 'web',
	deployment_id: '42',
	parent_snapshot_id: null,
	completed_at: '2026-01-01T01:01:00.000Z',
	delta_bytes: 2048,
	total_bytes: 2048,
	local_path: null,
	remote_key: null,
	storage_provider_id: null,
	checksum_sha256: null,
	error_message: null,
	notes: null,
	created_at: '2026-01-01T01:00:00.000Z'
};

function renderPage(snapshots = [fullSnapshot]) {
	render(SnapshotsPage, {
		props: {
			params: {},
			data: { snapshots }
		}
	});
}

describe('snapshots page', () => {
	beforeEach(() => {
		setUiContext.mockReset();
		clearUiContext.mockReset();
		snapshotsContext.mockClear();
		for (const fn of Object.values(api)) {
			fn.mockReset();
		}
		api.createSnapshot.mockResolvedValue({ ...fullSnapshot, id: 'snap_new', name: 'manual-2' });
		api.deleteSnapshot.mockResolvedValue(undefined);
		api.restoreSnapshot.mockResolvedValue({ message: 'Restore queued.' });
		api.rollbackAppToSnapshot.mockResolvedValue({ message: 'Rollback started.' });
		api.listStorageProviders.mockResolvedValue([
			{ id: 'provider_1', name: 'S3 Bucket', kind: 's3' }
		]);
		api.getBackupConfig.mockResolvedValue({
			enabled: true,
			full_schedule_cron: '0 3 * * *',
			full_retention_count: 7,
			pre_deploy_enabled: true,
			pre_deploy_retention_per_app: 5,
			keep_local: true,
			storage_provider_id: 'provider_1'
		});
		api.updateBackupConfig.mockResolvedValue(undefined);
	});

	it('renders snapshots from route data without initial loading state', () => {
		renderPage();

		expect(screen.queryByText(/loading/i)).not.toBeInTheDocument();
		expect(screen.getByText(/manual · full/i)).toBeInTheDocument();
	});

	it('creates a snapshot and shows a success banner', async () => {
		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /create snapshot/i }));

		expect(api.createSnapshot).toHaveBeenCalledOnce();
		expect(await screen.findByText(/snapshot created: manual-2/i)).toBeInTheDocument();
		expect(screen.getByText(/manual-2/i)).toBeInTheDocument();
	});

	it('deletes a snapshot from the timeline', async () => {
		const user = userEvent.setup();
		renderPage([fullSnapshot, preDeploySnapshot]);

		await user.click(screen.getByRole('button', { name: /delete snapshot before-deploy/i }));

		expect(api.deleteSnapshot).toHaveBeenCalledWith('snap_1');
		expect(screen.queryByText(/manual · full/i)).not.toBeInTheDocument();
	});

	it('confirms and restores a full snapshot', async () => {
		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /restore/i }));
		await user.click(screen.getByRole('button', { name: /yes, restore/i }));

		expect(api.restoreSnapshot).toHaveBeenCalledWith('snap_1');
		expect(await screen.findByText(/restore queued/i)).toBeInTheDocument();
	});

	it('rolls back an app from a pre-deploy snapshot', async () => {
		const user = userEvent.setup();
		renderPage([preDeploySnapshot]);

		await user.click(screen.getByRole('button', { name: /rollback/i }));
		await user.clear(screen.getByPlaceholderText('web'));
		await user.type(screen.getByPlaceholderText('web'), 'web');
		await user.click(screen.getByRole('button', { name: /confirm/i }));

		expect(api.rollbackAppToSnapshot).toHaveBeenCalledWith('snap_2', 'web');
		expect(await screen.findByText(/rollback started/i)).toBeInTheDocument();
	});

	it('loads and saves backup configuration', async () => {
		const user = userEvent.setup();
		renderPage();

		await user.click(screen.getByRole('button', { name: /config/i }));

		expect(api.getBackupConfig).toHaveBeenCalledOnce();
		expect(api.listStorageProviders).toHaveBeenCalledOnce();
		expect(await screen.findByDisplayValue('0 3 * * *')).toBeInTheDocument();

		await user.clear(screen.getByLabelText(/schedule \(cron\)/i));
		await user.type(screen.getByLabelText(/schedule \(cron\)/i), '0 4 * * *');
		await user.click(screen.getByRole('button', { name: /save config/i }));

		expect(api.updateBackupConfig).toHaveBeenCalledWith({
			enabled: true,
			full_schedule_cron: '0 4 * * *',
			full_retention_count: 7,
			pre_deploy_enabled: true,
			pre_deploy_retention_per_app: 5,
			keep_local: true,
			storage_provider_id: 'provider_1'
		});
		expect(await screen.findByText(/backup configuration saved/i)).toBeInTheDocument();
	});
});
