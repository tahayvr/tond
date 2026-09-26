import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, DatabaseAccess } from '$lib/api';
import DatabaseAccessCard from './database-access.svelte';

const api = vi.hoisted(() => ({
	getDatabaseAccess: vi.fn(),
	linkDatabaseApp: vi.fn(),
	unlinkDatabaseApp: vi.fn(),
	listApps: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const ref = (id: string, name: string, project_id: string | null = null) => ({
	id,
	name,
	status: 'running',
	project_id
});

function access(partial: Partial<DatabaseAccess> = {}): DatabaseAccess {
	return {
		database_id: 'db_1',
		project: null,
		project_apps: [],
		linked_apps: [],
		internal_host: 'main-db',
		internal_port: 5432,
		...partial
	};
}

const app = (id: string, name: string, source_type = 'git') => ({ id, name, source_type }) as App;

describe('DatabaseAccess', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		document.body.style.pointerEvents = '';
		api.listApps.mockResolvedValue([app('app_1', 'web'), app('app_2', 'api')]);
	});

	it('lists project members and linked apps with the hostname to use', async () => {
		api.getDatabaseAccess.mockResolvedValue(
			access({
				project: { id: 'p1', name: 'shop' },
				project_apps: [ref('app_2', 'api', 'p1')],
				linked_apps: [ref('app_1', 'web')]
			})
		);
		render(DatabaseAccessCard, { props: { databaseId: 'db_1', databaseName: 'main-db' } });

		expect(await screen.findByText('main-db:5432')).toBeInTheDocument();
		expect(screen.getByText(/apps in shop share its network/)).toBeInTheDocument();
		expect(screen.getByText('via shop')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'web' })).toHaveAttribute('href', '/apps/app_1');
		// Project members can't be unlinked; linked apps can.
		expect(screen.queryByRole('button', { name: 'Unlink api' })).not.toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Unlink web' })).toBeInTheDocument();
	});

	it('says plainly when no app can reach the database', async () => {
		api.getDatabaseAccess.mockResolvedValue(access());
		render(DatabaseAccessCard, { props: { databaseId: 'db_1', databaseName: 'main-db' } });
		expect(await screen.findByText('No app can reach main-db yet.')).toBeInTheDocument();
	});

	it('asks before unlinking and shows the result', async () => {
		api.getDatabaseAccess.mockResolvedValue(access({ linked_apps: [ref('app_1', 'web')] }));
		api.unlinkDatabaseApp.mockResolvedValue(access());
		const user = userEvent.setup();
		render(DatabaseAccessCard, { props: { databaseId: 'db_1', databaseName: 'main-db' } });

		await user.click(await screen.findByRole('button', { name: 'Unlink web' }));
		const dialog = await screen.findByRole('dialog');
		expect(
			within(dialog).getByText(/loses its connection to main-db right away/)
		).toBeInTheDocument();
		expect(api.unlinkDatabaseApp).not.toHaveBeenCalled();

		await user.click(within(dialog).getByRole('button', { name: 'Unlink' }));
		expect(api.unlinkDatabaseApp).toHaveBeenCalledWith('db_1', 'app_1');
		expect(await screen.findByText('web can no longer reach main-db.')).toBeInTheDocument();
	});

	it('shows the server error when unlinking fails and keeps the app listed', async () => {
		api.getDatabaseAccess.mockResolvedValue(access({ linked_apps: [ref('app_1', 'web')] }));
		api.unlinkDatabaseApp.mockRejectedValue(new Error('Docker error: daemon unavailable'));
		const user = userEvent.setup();
		render(DatabaseAccessCard, { props: { databaseId: 'db_1', databaseName: 'main-db' } });

		await user.click(await screen.findByRole('button', { name: 'Unlink web' }));
		const dialog = await screen.findByRole('dialog');
		await user.click(within(dialog).getByRole('button', { name: 'Unlink' }));
		await waitFor(() =>
			expect(within(dialog).getByRole('alert')).toHaveTextContent('daemon unavailable')
		);
		expect(screen.getByRole('link', { name: 'web' })).toBeInTheDocument();
	});

	it('offers a retry when access cannot be loaded', async () => {
		api.getDatabaseAccess.mockRejectedValueOnce(new Error('Server unavailable'));
		api.getDatabaseAccess.mockResolvedValueOnce(access());
		const user = userEvent.setup();
		render(DatabaseAccessCard, { props: { databaseId: 'db_1', databaseName: 'main-db' } });

		expect(await screen.findByText('Server unavailable')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /retry/i }));
		expect(await screen.findByText('No app can reach main-db yet.')).toBeInTheDocument();
	});
});
