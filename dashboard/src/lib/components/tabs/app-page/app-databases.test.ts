import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppDatabaseAccess, ManagedDatabase } from '$lib/api';
import AppDatabases from './app-databases.svelte';

const api = vi.hoisted(() => ({
	listAppDatabases: vi.fn(),
	listDatabases: vi.fn(),
	listProjects: vi.fn(),
	linkDatabaseApp: vi.fn(),
	unlinkDatabaseApp: vi.fn()
}));

const auth = vi.hoisted(() => ({ canWrite: true }));

vi.mock('$lib/auth.svelte', () => ({
	can: (scope: string) => auth.canWrite || !scope.endsWith(':write'),
	isAdmin: () => auth.canWrite
}));

const confirm = vi.hoisted(() => ({
	answer: true,
	confirmAction: vi.fn()
}));

vi.mock('$lib/confirm.svelte', () => ({
	// Like ConfirmHost: run onconfirm when the user confirms.
	confirmAction: confirm.confirmAction
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const reach = (
	id: string,
	name: string,
	via: 'project' | 'link',
	db_type = 'postgres'
): AppDatabaseAccess => ({
	id,
	name,
	db_type,
	status: 'running',
	via,
	internal_host: name,
	internal_port: db_type === 'redis' ? 6379 : 5432
});

const db = (id: string, name: string) => ({ id, name, db_type: 'postgres' }) as ManagedDatabase;

function renderTab(props: Partial<{ sourceType: string; projectId: string | null }> = {}) {
	render(AppDatabases, {
		props: {
			appId: 'app_1',
			appName: 'web',
			sourceType: props.sourceType ?? 'git',
			projectId: props.projectId ?? null
		}
	});
}

describe('AppDatabases', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
		document.body.style.pointerEvents = '';
		auth.canWrite = true;
		confirm.answer = true;
		confirm.confirmAction.mockReset();
		confirm.confirmAction.mockImplementation(async (o: { onconfirm?: () => unknown }) => {
			if (!confirm.answer) return false;
			await o.onconfirm?.();
			return true;
		});
		api.listDatabases.mockResolvedValue([db('db_1', 'main'), db('db_2', 'cache')]);
		api.listProjects.mockResolvedValue([{ id: 'p1', name: 'shop' }]);
	});

	it('lists databases reached through the project and through links', async () => {
		api.listAppDatabases.mockResolvedValue([
			reach('db_1', 'main', 'project'),
			reach('db_2', 'cache', 'link', 'redis')
		]);
		renderTab({ projectId: 'p1' });

		expect(await screen.findByText('via shop')).toBeInTheDocument();
		expect(screen.getByText('cache:6379')).toBeInTheDocument();
		expect(screen.getByText(/shared with the other apps in shop/)).toBeInTheDocument();
		expect(screen.queryByRole('button', { name: 'Unlink main' })).not.toBeInTheDocument();
		expect(screen.getByRole('button', { name: 'Unlink cache' })).toBeInTheDocument();
	});

	it('explains the empty state and the network model', async () => {
		api.listAppDatabases.mockResolvedValue([]);
		renderTab();
		expect(await screen.findByText("web can't reach any database.")).toBeInTheDocument();
		expect(screen.getByText(/Nothing else on\s+this server can reach it/)).toBeInTheDocument();
	});

	it('unlinks after confirmation', async () => {
		api.listAppDatabases
			.mockResolvedValueOnce([reach('db_2', 'cache', 'link', 'redis')])
			.mockResolvedValueOnce([]);
		api.unlinkDatabaseApp.mockResolvedValue({});
		const user = userEvent.setup();
		renderTab();

		await user.click(await screen.findByRole('button', { name: 'Unlink cache' }));
		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({ title: 'Unlink cache?', destructive: true })
		);
		expect(api.unlinkDatabaseApp).toHaveBeenCalledWith('db_2', 'app_1');
		expect(await screen.findByText('web can no longer reach cache.')).toBeInTheDocument();
	});

	it('explains that compose apps cannot be linked and loads nothing', async () => {
		renderTab({ sourceType: 'compose' });
		expect(screen.getByText(/Compose apps run on their own compose network/)).toBeInTheDocument();
		expect(api.listAppDatabases).not.toHaveBeenCalled();
	});
});
