import { render, screen, waitFor } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Template, TemplateDeployRequest } from '$lib/api';
import templatesJson from '../../../mocks/templates.json';

const auth = vi.hoisted(() => ({ admin: true }));
vi.mock('$lib/auth.svelte', () => ({ can: () => auth.admin }));

const { deployTemplate } = vi.hoisted(() => ({ deployTemplate: vi.fn() }));
vi.mock('$lib/api', async (importOriginal) => ({
	...(await importOriginal<typeof import('$lib/api')>()),
	deployTemplate
}));

import { ApiError } from '$lib/api';
import TemplateDeployForm from './template-deploy-form.svelte';
import TemplateCredentials from './template-credentials.svelte';

const catalog = templatesJson as Template[];
const byId = (id: string) => {
	const t = catalog.find((t) => t.id === id);
	if (!t) throw new Error(`no template ${id}`);
	return t;
};

function renderForm(id: string, availableMb = 1400) {
	const ondeployed = vi.fn();
	render(TemplateDeployForm, {
		props: {
			template: byId(id),
			memory: { total_mb: 2048, available_mb: availableMb },
			appsDomain: 'apps.example.com',
			projects: [],
			ondeployed
		}
	});
	return ondeployed;
}

const RESULT = {
	app_id: 'app_1',
	app_name: 'x',
	domain: null,
	deployment_id: 'dep_1',
	credentials: [],
	setup_notes: null
};

describe('TemplateDeployForm', () => {
	beforeEach(() => {
		deployTemplate.mockReset();
		deployTemplate.mockResolvedValue(RESULT);
	});

	it('deploys with the defaults and leaves generated secrets to the server', async () => {
		const user = userEvent.setup();
		const ondeployed = renderForm('vaultwarden');

		expect(screen.getByLabelText('App name')).toHaveValue('vaultwarden');
		expect(screen.getByText('vaultwarden.apps.example.com')).toBeInTheDocument();
		expect(screen.getByText(/1 secret is generated for you/i)).toBeInTheDocument();

		await user.click(screen.getByRole('button', { name: /deploy vaultwarden/i }));
		await waitFor(() => expect(ondeployed).toHaveBeenCalledWith(RESULT));
		expect(deployTemplate).toHaveBeenCalledWith('vaultwarden', {
			name: 'vaultwarden',
			inputs: {},
			allow_low_memory: false
		});
	});

	it('sends a custom domain, a changed default and an overridden secret', async () => {
		const user = userEvent.setup();
		renderForm('n8n');

		await user.clear(screen.getByLabelText('App name'));
		await user.type(screen.getByLabelText('App name'), 'flows');
		await user.type(screen.getByLabelText(/^domain/i), 'flows.example.com');
		await user.clear(screen.getByLabelText(/^time zone/i));
		await user.type(screen.getByLabelText(/^time zone/i), 'Europe/Berlin');
		await user.click(screen.getByRole('button', { name: /secret is generated/i }));
		await user.type(screen.getByLabelText('Encryption key'), 'my-own-key');

		await user.click(screen.getByRole('button', { name: /deploy n8n/i }));
		await waitFor(() => expect(deployTemplate).toHaveBeenCalled());
		const [, body] = deployTemplate.mock.calls[0] as [string, TemplateDeployRequest];
		expect(body).toEqual({
			name: 'flows',
			domain: 'flows.example.com',
			inputs: { GENERIC_TIMEZONE: 'Europe/Berlin', N8N_ENCRYPTION_KEY: 'my-own-key' },
			allow_low_memory: false
		});
	});

	it('asks for required inputs before deploying', async () => {
		const user = userEvent.setup();
		const ondeployed = renderForm('pocketbase');

		await user.click(screen.getByRole('button', { name: /deploy pocketbase/i }));
		expect(await screen.findByText('Superuser email is required.')).toBeInTheDocument();
		expect(deployTemplate).not.toHaveBeenCalled();

		await user.type(screen.getByLabelText('Superuser email'), 'nope');
		expect(await screen.findByText(/must be an email address/)).toBeInTheDocument();
		await user.clear(screen.getByLabelText('Superuser email'));
		await user.type(screen.getByLabelText('Superuser email'), 'me@example.com');
		await user.click(screen.getByRole('button', { name: /deploy pocketbase/i }));
		await waitFor(() => expect(ondeployed).toHaveBeenCalled());
		expect(deployTemplate.mock.calls[0][1].inputs).toEqual({ SUPERUSER_EMAIL: 'me@example.com' });
	});

	it('warns when the template needs more memory than is free, and deploys only when told to', async () => {
		const user = userEvent.setup();
		renderForm('immich', 1000);

		expect(screen.getByRole('alert')).toHaveTextContent(
			/1000 MB of memory free and Immich needs about 1.6 GB/
		);
		const deploy = screen.getByRole('button', { name: /deploy immich/i });
		expect(deploy).toBeDisabled();
		await user.click(screen.getByLabelText('Deploy anyway'));
		expect(deploy).toBeEnabled();
		await user.click(deploy);
		await waitFor(() => expect(deployTemplate).toHaveBeenCalled());
		expect(deployTemplate.mock.calls[0][1].allow_low_memory).toBe(true);
	});

	it('shows the server error and asks again when memory ran out meanwhile', async () => {
		deployTemplate.mockRejectedValue(
			new ApiError(
				'Umami needs about 300 MB and this server has 120 MB free.',
				409,
				null,
				'insufficient_memory'
			)
		);
		const user = userEvent.setup();
		const ondeployed = renderForm('umami');
		await user.click(screen.getByRole('button', { name: /deploy umami/i }));
		expect(await screen.findByText(/this server has 120 MB free/)).toBeInTheDocument();
		expect(ondeployed).not.toHaveBeenCalled();
		expect(screen.getByRole('button', { name: /deploy umami/i })).toBeEnabled();
	});

	it('explains who can deploy when the user may not', () => {
		auth.admin = false;
		try {
			renderForm('umami');
			expect(screen.queryByRole('button', { name: /deploy umami/i })).not.toBeInTheDocument();
			expect(screen.getByText(/needs an admin role/)).toBeInTheDocument();
		} finally {
			auth.admin = true;
		}
	});
});

describe('TemplateCredentials', () => {
	it('shows the credentials once and links to the live deploy', () => {
		render(TemplateCredentials, {
			props: {
				templateName: 'Grafana',
				result: {
					app_id: 'app_1',
					app_name: 'grafana',
					domain: 'grafana.apps.example.com',
					deployment_id: 'dep_1',
					credentials: [{ key: 'ADMIN_PASSWORD', label: 'Admin password', value: 's3cret-value' }],
					setup_notes: 'Sign in as admin.'
				}
			}
		});
		expect(screen.getByText(/shown only this once/i)).toBeInTheDocument();
		expect(screen.getByLabelText('Admin password')).toHaveValue('s3cret-value');
		expect(screen.getByText('Sign in as admin.')).toBeInTheDocument();
		expect(screen.getByRole('link', { name: /watch the grafana deploy/i })).toHaveAttribute(
			'href',
			'/apps/app_1/deployments/dep_1'
		);
	});
});
