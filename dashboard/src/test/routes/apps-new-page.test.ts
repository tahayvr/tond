import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
	page: {
		url: new URL('http://localhost/apps/new'),
		params: {}
	}
}));

const {
	goto,
	createApp,
	createDatabase,
	createProject,
	deployApp,
	deleteApp,
	deleteDatabase,
	deleteProject,
	getSystemMetrics,
	listRemoteBranches,
	inspectImage,
	listTemplates,
	listProjects,
	listRegistryCredentials,
	setUiContext,
	clearUiContext,
	projectsContext
} = vi.hoisted(() => ({
	goto: vi.fn(),
	createApp: vi.fn(),
	createDatabase: vi.fn(),
	createProject: vi.fn(),
	deployApp: vi.fn(),
	deleteApp: vi.fn(),
	deleteDatabase: vi.fn(),
	deleteProject: vi.fn(),
	getSystemMetrics: vi.fn(),
	listRemoteBranches: vi.fn(),
	inspectImage: vi.fn(),
	listTemplates: vi.fn(),
	listProjects: vi.fn(),
	listRegistryCredentials: vi.fn(),
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	projectsContext: vi.fn(() => ({ type: 'projects' }))
}));

vi.mock('$lib/auth.svelte', () => ({ isAdmin: () => true }));

vi.mock('$app/navigation', () => ({
	goto
}));

vi.mock('$app/state', () => ({
	page: state.page
}));

vi.mock('$lib/api', () => ({
	createApp,
	createDatabase,
	createProject,
	deployApp,
	deleteApp,
	deleteDatabase,
	deleteProject,
	getSystemMetrics,
	listRemoteBranches,
	inspectImage,
	listTemplates,
	listProjects,
	listRegistryCredentials
}));

vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext,
	clearUiContext,
	projectsContext
}));

import NewAppPage from '../../../src/routes/apps/new/+page.svelte';

const data = {
	projects: [
		{
			id: 'proj_1',
			name: 'Existing Project',
			description: 'Already created',
			env: [],
			active_preview_count: 0,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z'
		}
	],
	registryCredentials: [],
	templates: [
		{
			id: 'plausible',
			name: 'Plausible Analytics',
			description: 'Self-hosted privacy-friendly analytics',
			category: 'analytics',
			tags: ['analytics'],
			maintainers: ['tond'],
			project: {
				name: 'plausible',
				description: 'Plausible app and supporting services'
			},
			source: {
				kind: 'image',
				url: 'ghcr.io/plausible/community-edition',
				ref: 'latest'
			},
			deploy: {
				method: 'image',
				default_port: 8000,
				health_check_path: '/',
				volumes: [{ container_path: '/var/lib/plausible', description: 'Persistent app data' }]
			},
			required_env: [
				{ key: 'BASE_URL', description: 'Public URL', required: true, value: null },
				{
					key: 'DATABASE_URL',
					description: 'Primary PostgreSQL connection string',
					required: true,
					value: '${db.plausible-db.internal_connection_string}'
				}
			],
			optional_env: [],
			dependencies: [
				{ name: 'plausible-db', kind: 'database', engine: 'postgres', required: true }
			],
			recommended_domains: ['stats.example.com'],
			post_deploy_checks: [],
			setup_notes: 'Configure BASE_URL first.'
		}
	]
};

describe('new app page', () => {
	beforeEach(() => {
		state.page.url = new URL('http://localhost/apps/new');
		goto.mockReset();
		createApp.mockReset();
		createDatabase.mockReset();
		createProject.mockReset();
		deployApp.mockReset();
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();

		createProject.mockResolvedValue({
			id: 'proj_template',
			name: 'plausible',
			description: 'Plausible app and supporting services',
			env: [],
			active_preview_count: 0,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z'
		});
		createDatabase.mockResolvedValue({
			id: 'db_1',
			name: 'plausible-db',
			db_type: 'postgres',
			project_id: 'proj_template',
			host_port: 5401,
			status: 'running',
			db_name: 'plausible',
			db_user: 'plausible',
			has_password: true,
			has_root_password: false,
			memory_limit: null,
			custom_external_url: null,
			custom_internal_url: null,
			image_used: 'postgres:16-alpine',
			custom_image: null,
			created_at: '2026-01-01T00:00:00.000Z',
			updated_at: '2026-01-01T00:00:00.000Z',
			public_access: false,
			connection_string: 'postgres://plausible:••••••••@localhost:5401/plausible',
			public_connection_string: null,
			internal_connection_string:
				'postgres://plausible:••••••••@tond-db-plausible-db:5432/plausible',
			// Creation returns the generated credentials once; the template uses them.
			credentials: {
				db_user: 'plausible',
				db_password: 'secret',
				root_password: null,
				connection_string: 'postgres://plausible:secret@localhost:5401/plausible',
				public_connection_string: null,
				internal_connection_string:
					'postgres://plausible:secret@tond-db-plausible-db:5432/plausible'
			}
		});
		createApp.mockResolvedValue({ id: 'app_created', name: 'plausible-app' });
		deployApp.mockResolvedValue({
			message: 'Deployment started',
			app_id: 'app_created',
			deployment_id: 'dep_first'
		});
		for (const fn of [deleteApp, deleteDatabase, deleteProject]) {
			fn.mockReset();
			fn.mockResolvedValue(undefined);
		}
		getSystemMetrics.mockReset();
		getSystemMetrics.mockResolvedValue({ mem_total: 4 * 1024 ** 3 });
		listRemoteBranches.mockReset();
		listRemoteBranches.mockResolvedValue({
			default_branch: 'trunk',
			branches: ['trunk', 'develop'],
			truncated: false
		});
		inspectImage.mockReset();
		inspectImage.mockResolvedValue({
			image: 'nginx:alpine',
			exposed_ports: [80],
			env_port: null,
			suggested_port: 80,
			pulled: true
		});
	});

	it('validates the source step before advancing', async () => {
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'My App');
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		expect(await screen.findByText(/git url is required/i)).toBeInTheDocument();
		expect(screen.getByText(/configuration error/i)).toBeInTheDocument();
	});

	it('creates a template app with dependency interpolation and opens the live deploy view', async () => {
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'Plausible App');
		await user.click(screen.getByRole('button', { name: /templates/i }));
		await user.click(screen.getByRole('button', { name: /plausible analytics/i }));
		await user.type(screen.getByLabelText('BASE_URL'), 'https://stats.example.com');

		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		await screen.findByText(/deploy application/i);
		await user.click(screen.getByRole('button', { name: /deploy application/i }));

		await waitFor(() => {
			expect(createProject).toHaveBeenCalledWith({
				name: 'plausible',
				description: 'Plausible app and supporting services'
			});
		});

		expect(createDatabase).toHaveBeenCalledWith({
			name: 'plausible-db',
			db_type: 'postgres',
			project_id: 'proj_template'
		});
		expect(createApp).toHaveBeenCalledWith(
			expect.objectContaining({
				name: 'plausible-app',
				source_type: 'image',
				source_url: 'ghcr.io/plausible/community-edition',
				project_id: 'proj_template',
				container_port: 8000,
				domain: 'stats.example.com',
				health_check_path: '/',
				env_vars: {
					BASE_URL: 'https://stats.example.com',
					DATABASE_URL: 'postgres://plausible:secret@tond-db-plausible-db:5432/plausible'
				}
			})
		);
		expect(deployApp).toHaveBeenCalledWith('app_created');
		expect(goto).toHaveBeenCalledWith('/apps/app_created/deployments/dep_first');
		expect(deleteDatabase).not.toHaveBeenCalled();
	});

	it('removes the project and databases a failed template deploy created', async () => {
		createApp.mockRejectedValue(new Error('App name already taken'));
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'Plausible App');
		await user.click(screen.getByRole('button', { name: /templates/i }));
		await user.click(screen.getByRole('button', { name: /plausible analytics/i }));
		await user.type(screen.getByLabelText('BASE_URL'), 'https://stats.example.com');
		for (let i = 0; i < 4; i++) {
			await user.click(screen.getByRole('button', { name: /^next$/i }));
		}
		await user.click(await screen.findByRole('button', { name: /deploy application/i }));

		expect(await screen.findByText(/nothing was left behind/i)).toBeInTheDocument();
		expect(deleteDatabase).toHaveBeenCalledWith('db_1');
		expect(deleteProject).toHaveBeenCalledWith('proj_template');
		expect(deleteApp).not.toHaveBeenCalled();
		expect(goto).not.toHaveBeenCalled();
	});

	it('names what it could not clean up', async () => {
		createApp.mockRejectedValue(new Error('Boom'));
		deleteProject.mockRejectedValue(new Error('project in use'));
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'Plausible App');
		await user.click(screen.getByRole('button', { name: /templates/i }));
		await user.click(screen.getByRole('button', { name: /plausible analytics/i }));
		await user.type(screen.getByLabelText('BASE_URL'), 'https://stats.example.com');
		for (let i = 0; i < 4; i++) {
			await user.click(screen.getByRole('button', { name: /^next$/i }));
		}
		await user.click(await screen.findByRole('button', { name: /deploy application/i }));

		expect(
			await screen.findByText(/couldn't remove what it had created: project plausible/i)
		).toBeInTheDocument();
	});

	it('asks for the port of an image app and detects it from the image', async () => {
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'web');
		await user.click(screen.getByRole('button', { name: /docker/i }));
		await user.click(screen.getByRole('button', { name: /docker image/i }));
		await user.type(screen.getByLabelText(/^docker image/i), 'nginx:alpine');
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		const port = (await screen.findByLabelText(/container port/i)) as HTMLInputElement;
		await waitFor(() => expect(port.value).toBe('80'));
		expect(inspectImage).toHaveBeenCalledWith('nginx:alpine', undefined);
		expect(screen.getByText(/the image exposes 80/i)).toBeInTheDocument();
	});

	it('picks the repository default branch and deploys it', async () => {
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'my-app');
		await user.type(
			screen.getByLabelText(/git repository url/i),
			'https://github.com/example/repo.git'
		);
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		const branch = (await screen.findByLabelText(/branch/i)) as HTMLInputElement;
		await waitFor(() => expect(branch.value).toBe('trunk'));
		expect(listRemoteBranches).toHaveBeenCalledWith('https://github.com/example/repo.git', {});

		for (let i = 0; i < 4; i++) {
			await user.click(screen.getByRole('button', { name: /^next$/i }));
		}
		await user.click(await screen.findByRole('button', { name: /deploy application/i }));
		await waitFor(() =>
			expect(createApp).toHaveBeenCalledWith(
				expect.objectContaining({ branch: 'trunk', watch_branch: 'trunk', container_port: 8080 })
			)
		);
		const body = createApp.mock.calls[0][0];
		expect(body.memory_limit).toBeUndefined();
		expect(body.cpu_limit).toBeUndefined();
		expect(body.health_check_path).toBeUndefined();
	});

	it('shows an inline error when app creation fails', async () => {
		createApp.mockRejectedValue(new Error('Create app failed'));

		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'my-app');
		await user.type(
			screen.getByLabelText(/git repository url/i),
			'https://github.com/example/repo.git'
		);

		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		await user.click(screen.getByRole('button', { name: /deploy application/i }));

		expect(await screen.findByText(/create app failed/i)).toBeInTheDocument();
		expect(goto).not.toHaveBeenCalled();
	});
});
