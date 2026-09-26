import { render, screen, waitFor, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, ComposeService } from '$lib/api';
import { MOCK_COMPOSE_APP, MOCK_COMPOSE_SERVICES } from '../../../mocks/compose-fixtures';
import ComposeServices from './compose-services.svelte';
import ServiceRoutingDialog from './service-routing-dialog.svelte';
import DeploymentServices from './deployment-services.svelte';

const api = vi.hoisted(() => ({
	listServices: vi.fn(),
	serviceAction: vi.fn(),
	updateService: vi.fn(),
	serviceLogs: vi.fn()
}));

const confirm = vi.hoisted(() => ({ confirmAction: vi.fn() }));
vi.mock('$lib/confirm.svelte', () => ({ confirmAction: confirm.confirmAction }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const app: App = structuredClone(MOCK_COMPOSE_APP);
const services = (): ComposeService[] => structuredClone(MOCK_COMPOSE_SERVICES.app_compose);

beforeEach(() => {
	vi.clearAllMocks();
	api.listServices.mockResolvedValue(services());
	api.serviceLogs.mockResolvedValue({ service: 'web', lines: [] });
	confirm.confirmAction.mockImplementation(async (o: { onconfirm?: () => unknown }) => {
		await o.onconfirm?.();
		return true;
	});
});

describe('ComposeServices', () => {
	it('lists every service with its state, image, ports and domains', async () => {
		render(ComposeServices, { props: { app, canControl: true, canEdit: true, canReadLogs: true } });
		const list = await screen.findByRole('list', { name: 'Services' });
		const items = within(list).getAllByRole('listitem', { name: /./ });
		expect(items.map((i) => within(i).getByRole('heading').textContent)).toEqual([
			'web',
			'admin',
			'cache',
			'db',
			'migrate'
		]);
		const web = items[0];
		expect(within(web).getByText('Running')).toBeInTheDocument();
		expect(within(web).getByText('3000 → 127.0.0.1:8300')).toBeInTheDocument();
		expect(within(web).getByRole('link', { name: /www\.shop\.example\.com/ })).toHaveAttribute(
			'href',
			'https://www.shop.example.com/'
		);
		expect(within(items[2]).getByText('Healthy')).toBeInTheDocument();
		expect(within(items[2]).getByText('Internal only')).toBeInTheDocument();
		expect(within(items[4]).getByText('Done')).toBeInTheDocument();
		expect(screen.getByText('All 5 services running')).toBeInTheDocument();
	});

	it('restarts one service and confirms before stopping one', async () => {
		const user = userEvent.setup();
		api.serviceAction.mockResolvedValue({ message: 'Service cache restarted', service: null });
		render(ComposeServices, { props: { app, canControl: true } });
		const list = await screen.findByRole('list', { name: 'Services' });
		const cache = within(list).getAllByRole('listitem', { name: /./ })[2];
		await user.click(within(cache).getByRole('button', { name: 'Restart' }));
		expect(api.serviceAction).toHaveBeenCalledWith('app_compose', 'cache', 'restart');
		expect(await screen.findByText('Service cache restarted')).toBeInTheDocument();

		await user.click(within(cache).getByRole('button', { name: 'Stop' }));
		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({ title: 'Stop cache?', destructive: true })
		);
		expect(api.serviceAction).toHaveBeenCalledWith('app_compose', 'cache', 'stop');
	});

	it('hides controls without the permissions and prompts a deploy after a port change', async () => {
		const pending = services();
		pending[1] = { ...pending[1], redeploy_required: true };
		api.listServices.mockResolvedValue(pending);
		const onDeploy = vi.fn();
		const user = userEvent.setup();
		render(ComposeServices, { props: { app, canDeploy: true, onDeploy } });
		await screen.findByRole('list', { name: 'Services' });
		expect(screen.queryByRole('button', { name: 'Restart' })).not.toBeInTheDocument();
		expect(screen.queryByRole('button', { name: /Domains/ })).not.toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Deploy now' }));
		expect(onDeploy).toHaveBeenCalled();
	});

	it('summarises services on the overview', async () => {
		render(ComposeServices, { props: { app, compact: true } });
		expect(await screen.findByText('All 5 services running')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: 'Manage services' })).toHaveAttribute(
			'href',
			'/apps/app_compose?tab=services'
		);
	});
});

describe('ServiceRoutingDialog', () => {
	it('publishes a port, adds a normalised domain and saves', async () => {
		const user = userEvent.setup();
		const cache = services()[2];
		const onSaved = vi.fn();
		api.updateService.mockResolvedValue({
			service: { ...cache, published_port: 6379, domains: ['cache.example.com'] },
			port_changed: true,
			redeploy_required: true,
			warnings: []
		});
		render(ServiceRoutingDialog, {
			props: { open: true, appId: 'app_compose', service: cache, onSaved }
		});
		const input = screen.getByLabelText('Domains');
		expect(input).toBeDisabled();
		await user.click(screen.getByRole('radio', { name: 'Other…' }));
		await user.type(screen.getByLabelText('Port the service listens on'), '6379');
		expect(input).toBeEnabled();
		await user.type(input, 'https://Cache.Example.com/{Enter}');
		expect(screen.getByText('cache.example.com')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: 'Save' }));
		expect(api.updateService).toHaveBeenCalledWith('app_compose', 'cache', {
			published_port: 6379,
			domains: ['cache.example.com']
		});
		expect(onSaved).toHaveBeenCalled();
		expect(await screen.findByText(/bound on the next deploy/)).toBeInTheDocument();
	});

	it('refuses something that is not a domain', async () => {
		const user = userEvent.setup();
		render(ServiceRoutingDialog, {
			props: { open: true, appId: 'app_compose', service: services()[0], onSaved: vi.fn() }
		});
		await user.type(screen.getByLabelText('Domains'), 'not a domain{Enter}');
		expect(await screen.findByRole('alert')).toHaveTextContent("doesn't look like a domain");
		await waitFor(() => expect(api.updateService).not.toHaveBeenCalled());
	});
});

describe('DeploymentServices', () => {
	it('shows how each image was made and where each service is', () => {
		render(DeploymentServices, {
			props: {
				services: [
					{
						service: 'admin',
						source: 'build',
						source_image: '/srv/admin',
						image: 'tond-shop/admin:abc',
						digest: 'sha256:8a0d91b7cc3e2f1b',
						status: 'building'
					},
					{
						service: 'cache',
						source: 'pull',
						source_image: 'redis:7',
						image: 'tond-shop/cache:abc',
						digest: null,
						status: 'healthy'
					}
				]
			}
		});
		expect(screen.getByText('Building')).toBeInTheDocument();
		expect(screen.getByText('Ready')).toBeInTheDocument();
		expect(screen.getByText('tond-shop/admin:abc')).toBeInTheDocument();
		expect(screen.getByText('redis:7')).toBeInTheDocument();
	});
});
