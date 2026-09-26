import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { ManagedDatabaseDetail } from '$lib/api';

const api = vi.hoisted(() => ({
	getDatabase: vi.fn(),
	stopDatabase: vi.fn(),
	startDatabase: vi.fn(),
	deleteDatabase: vi.fn(),
	updateDatabase: vi.fn(),
	updateDatabaseCredentials: vi.fn(),
	redeployDatabase: vi.fn(),
	listProjects: vi.fn()
}));

vi.mock('$lib/api', () => ({ ...api }));
vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$app/state', () => ({
	page: { url: new URL('http://localhost/databases/db_pg1'), params: { id: 'db_pg1' } }
}));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	databaseContext: vi.fn((database) => ({ type: 'database', database })),
	openFeedPanel: vi.fn()
}));

import DatabasePage from '../../routes/databases/[id]/+page.svelte';

const pg: ManagedDatabaseDetail = {
	id: 'db_pg1',
	name: 'app-postgres',
	db_type: 'postgres',
	project_id: null,
	host_port: 5401,
	status: 'running',
	db_name: 'app_db',
	db_user: 'app_user',
	db_password: 'old_password',
	root_password: null,
	memory_limit: null,
	custom_external_url: null,
	custom_internal_url: null,
	image_used: 'postgres:16-alpine',
	custom_image: null,
	created_at: '2026-01-01T00:00:00.000Z',
	updated_at: '2026-01-01T00:00:00.000Z',
	connection_string: null,
	internal_connection_string: null
};

async function openEnvironment(db: ManagedDatabaseDetail) {
	const user = userEvent.setup();
	render(DatabasePage, { props: { params: { id: db.id }, data: { db, projects: [] } } });
	await user.click(screen.getByRole('tab', { name: /environment/i }));
	return user;
}

describe('databases/[id] credentials', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		document.body.style.pointerEvents = '';
	});

	it('explains that password changes apply inside the running database', async () => {
		await openEnvironment(pg);

		expect(screen.getByText(/updates it inside the running database/i)).toBeInTheDocument();
		expect(screen.queryByText(/redeploy required/i)).not.toBeInTheDocument();
		// Only the password is editable; the name and user can't change after creation.
		expect(screen.getAllByRole('button', { name: /^change /i })).toHaveLength(1);
	});

	it('changes the password and reports success', async () => {
		api.updateDatabaseCredentials.mockResolvedValue({
			database: { ...pg, db_password: 'new_password' },
			changed: ['db_password']
		});
		const user = await openEnvironment(pg);

		await user.click(screen.getByRole('button', { name: /change password/i }));
		const dialog = await screen.findByRole('dialog');
		await user.type(within(dialog).getByLabelText(/new password/i), 'new_password');
		await user.click(within(dialog).getByRole('button', { name: /change password/i }));

		expect(api.updateDatabaseCredentials).toHaveBeenCalledWith('db_pg1', {
			db_password: 'new_password'
		});
		expect(await screen.findByText(/changed inside the database and saved/i)).toBeInTheDocument();
	});

	it('shows the server error in the dialog when the change fails', async () => {
		api.updateDatabaseCredentials.mockRejectedValue(
			new Error(
				"Couldn't change the password inside 'app-postgres': exit code 2. Nothing was changed."
			)
		);
		const user = await openEnvironment(pg);

		await user.click(screen.getByRole('button', { name: /change password/i }));
		const dialog = await screen.findByRole('dialog');
		await user.type(within(dialog).getByLabelText(/new password/i), 'x');
		await user.click(within(dialog).getByRole('button', { name: /change password/i }));

		expect(await within(dialog).findByRole('alert')).toHaveTextContent(/nothing was changed/i);
	});

	it('disables password changes while the database is stopped', async () => {
		await openEnvironment({ ...pg, status: 'stopped' });

		expect(screen.getByRole('button', { name: /change password/i })).toBeDisabled();
		expect(screen.getByText(/start the database to change a password/i)).toBeInTheDocument();
	});

	it('offers no password change for Redis', async () => {
		await openEnvironment({ ...pg, db_type: 'redis', db_name: null });

		expect(screen.queryByRole('button', { name: /^change /i })).not.toBeInTheDocument();
		expect(screen.getByText(/can't be changed in place yet/i)).toBeInTheDocument();
	});
});
