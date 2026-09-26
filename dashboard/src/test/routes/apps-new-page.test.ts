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
	deployApp,
	getSystemMetrics,
	listRemoteBranches,
	inspectImage,
	listProjects,
	listRegistryCredentials,
	setUiContext,
	clearUiContext,
	projectsContext
} = vi.hoisted(() => ({
	goto: vi.fn(),
	createApp: vi.fn(),
	deployApp: vi.fn(),
	getSystemMetrics: vi.fn(),
	listRemoteBranches: vi.fn(),
	inspectImage: vi.fn(),
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
	deployApp,
	getSystemMetrics,
	listRemoteBranches,
	inspectImage,
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
	registryCredentials: []
};

describe('new app page', () => {
	beforeEach(() => {
		state.page.url = new URL('http://localhost/apps/new');
		goto.mockReset();
		createApp.mockReset();
		deployApp.mockReset();
		setUiContext.mockReset();
		clearUiContext.mockReset();
		projectsContext.mockClear();

		createApp.mockResolvedValue({ id: 'app_created', name: 'plausible-app' });
		deployApp.mockResolvedValue({
			message: 'Deployment started',
			app_id: 'app_created',
			deployment_id: 'dep_first'
		});
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

	it('sends templates to the one-click catalog', async () => {
		render(NewAppPage, { props: { data, params: {} } });

		expect(screen.getByRole('link', { name: /templates/i })).toHaveAttribute('href', '/templates');
	});

	it('creates a background worker without a port or domain', async () => {
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });

		await user.type(screen.getByLabelText(/application name/i), 'jobs');
		await user.click(screen.getByRole('button', { name: /docker/i }));
		await user.click(screen.getByRole('button', { name: /docker image/i }));
		await user.type(screen.getByLabelText(/^docker image/i), 'acme/worker:1');
		await user.click(screen.getByRole('button', { name: /^next$/i }));

		await user.click(await screen.findByRole('button', { name: /background worker/i }));
		expect(screen.queryByLabelText(/container port/i)).not.toBeInTheDocument();
		expect(screen.queryByLabelText(/custom domain/i)).not.toBeInTheDocument();
		const secs = screen.getByLabelText(/stays up for/i);
		await user.clear(secs);
		await user.type(secs, '15');

		for (let i = 0; i < 3; i++) {
			await user.click(screen.getByRole('button', { name: /^next$/i }));
		}
		expect(await screen.findByText(/background worker \(ready after 15s/i)).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /deploy application/i }));
		await waitFor(() =>
			expect(createApp).toHaveBeenCalledWith(
				expect.objectContaining({ readiness: 'none', ready_after_secs: 15 })
			)
		);
		const body = createApp.mock.calls[0][0];
		expect(body.domain).toBeUndefined();
		expect(body.health_check_path).toBeUndefined();
	});

	it('explains a private image it cannot read yet instead of failing', async () => {
		inspectImage.mockResolvedValue({
			image: 'ghcr.io/acme/private:1',
			exposed_ports: [],
			env_port: null,
			suggested_port: null,
			pulled: false,
			unavailable:
				"Tond couldn't read ghcr.io/acme/private:1. If it's private, choose registry credentials on the Environment step and Tond reads its port then.",
			needs_credentials: true
		});
		const user = userEvent.setup();
		render(NewAppPage, { props: { data, params: {} } });
		await user.type(screen.getByLabelText(/application name/i), 'web');
		await user.click(screen.getByRole('button', { name: /docker/i }));
		await user.click(screen.getByRole('button', { name: /docker image/i }));
		await user.type(screen.getByLabelText(/^docker image/i), 'ghcr.io/acme/private:1');
		await user.click(screen.getByRole('button', { name: /^next$/i }));
		expect(
			await screen.findByText(/choose registry credentials on the environment step/i)
		).toBeInTheDocument();
		expect(screen.queryByText(/configuration error/i)).not.toBeInTheDocument();
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
