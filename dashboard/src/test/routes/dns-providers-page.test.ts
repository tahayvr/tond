import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DnsProvider } from '$lib/api';

const api = vi.hoisted(() => ({
	listDnsProviders: vi.fn(),
	createDnsProvider: vi.fn(),
	updateDnsProvider: vi.fn(),
	deleteDnsProvider: vi.fn(),
	testDnsProvider: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});
vi.mock('$lib/auth.svelte', () => ({ can: () => true }));
vi.mock('$lib/confirm.svelte', () => ({ confirmAction: vi.fn() }));
vi.mock('$lib/ui-context.svelte', () => ({
	setUiContext: vi.fn(),
	clearUiContext: vi.fn(),
	integrationsContext: vi.fn(() => ({ type: 'integrations' }))
}));

import DnsPage from '../../routes/integrations/dns/+page.svelte';

const cloudflare: DnsProvider = {
	id: 'dns_1',
	name: 'Cloudflare',
	kind: 'cloudflare',
	zones: ['example-apps.com'],
	config: {},
	token_set: true,
	created_at: '2026-09-01T00:00:00Z',
	updated_at: '2026-09-01T00:00:00Z',
	last_used_at: null,
	last_error: null
};

function renderPage(providers: DnsProvider[]) {
	return render(DnsPage, {
		props: {
			params: {},
			data: { providers, appsDomain: 'example-apps.com', previewDomain: 'example-previews.com' }
		}
	});
}

describe('DNS providers page', () => {
	beforeEach(() => {
		for (const fn of Object.values(api)) fn.mockReset();
	});

	it('shows which wildcard certificates a provider covers', () => {
		renderPage([cloudflare]);
		expect(screen.getByText(/issued and renewed through cloudflare/i)).toBeInTheDocument();
		expect(screen.getByText(/each preview gets its own certificate/i)).toBeInTheDocument();
	});

	it('adds a provider with scoped-token instructions and never echoes the token', async () => {
		api.createDnsProvider.mockResolvedValue({
			...cloudflare,
			id: 'dns_2',
			name: 'Hetzner DNS',
			kind: 'hetzner',
			zones: ['example-previews.com']
		});
		api.listDnsProviders.mockResolvedValue([cloudflare]);
		const user = userEvent.setup();
		renderPage([cloudflare]);

		await user.click(screen.getByRole('button', { name: /add provider/i }));
		await user.click(screen.getByRole('button', { name: 'Hetzner DNS' }));
		expect(screen.getByText(/generate a token with read & write permission/i)).toBeInTheDocument();
		// The uncovered preview domain is suggested as the zone.
		expect(screen.getByLabelText('Zones')).toHaveValue('example-previews.com');

		await user.click(screen.getByRole('button', { name: /^add provider$/i }));
		expect(screen.getByRole('alert')).toHaveTextContent(/paste the api token/i);

		await user.type(screen.getByLabelText('API token'), 'secret-token');
		await user.click(screen.getByRole('button', { name: /^add provider$/i }));
		expect(api.createDnsProvider).toHaveBeenCalledWith({
			name: 'Hetzner DNS',
			kind: 'hetzner',
			zones: ['example-previews.com'],
			token: 'secret-token',
			config: {}
		});
		expect(await screen.findByText(/hetzner dns added/i)).toBeInTheDocument();
		expect(screen.queryByDisplayValue('secret-token')).toBeNull();
	});

	it('shows per-zone test results', async () => {
		api.testDnsProvider.mockResolvedValue({
			ok: false,
			zones: [
				{
					zone: 'example-apps.com',
					ok: false,
					message: 'Cloudflare answered 403: Unauthorized — check the token and its permissions'
				}
			]
		});
		api.listDnsProviders.mockResolvedValue([cloudflare]);
		const user = userEvent.setup();
		renderPage([cloudflare]);
		await user.click(screen.getByRole('button', { name: 'Test' }));
		expect(await screen.findByText(/check the token and its permissions/i)).toBeInTheDocument();
	});
});
