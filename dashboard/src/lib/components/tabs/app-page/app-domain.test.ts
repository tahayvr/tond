import { render, screen, within } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, AppDomain, DomainCheck } from '$lib/api';
import AppDomainCard, {
	automaticAddress,
	dnsHeadline,
	isPlaceholderDomain
} from './app-domain.svelte';

const api = vi.hoisted(() => ({
	checkDomain: vi.fn(),
	getSettings: vi.fn(),
	listCertificates: vi.fn(),
	retryCertificate: vi.fn(),
	listAppDomains: vi.fn(),
	addAppDomain: vi.fn(),
	addWwwPair: vi.fn(),
	updateAppDomain: vi.fn(),
	removeAppDomain: vi.fn()
}));

const confirm = vi.hoisted(() => ({ confirmAction: vi.fn() }));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

vi.mock('$lib/confirm.svelte', () => confirm);

const app = {
	id: 'app_1',
	name: 'web',
	domain: 'app.example.com'
} as App;

function row(domain: string, kind: AppDomain['kind'], extra: Partial<AppDomain> = {}): AppDomain {
	return {
		domain,
		kind,
		redirect_to: null,
		redirect_code: 308,
		service: null,
		port: null,
		created_at: '2026-09-01T00:00:00Z',
		automatic: false,
		warning: null,
		...extra
	};
}

function check(
	domain = 'app.example.com',
	overrides: Partial<DomainCheck['dns']> = {}
): DomainCheck {
	return {
		domain,
		zone: 'example.com',
		apps_domain: 'apps.example.com',
		under_apps_domain: false,
		records: [{ type: 'A', name: 'app', host: domain, value: '203.0.113.10' }],
		wildcard_records: [],
		server_ips: ['203.0.113.10'],
		dns: {
			status: 'mismatch',
			resolved: ['198.51.100.1'],
			expected: ['203.0.113.10'],
			stray: [],
			message: `DNS for ${domain} points to 198.51.100.1, expected 203.0.113.10.`,
			...overrides
		},
		certificate: null,
		covered_by: null,
		dns_provider: null,
		checked_at: new Date().toISOString()
	};
}

describe('domain card helpers', () => {
	it('recognises placeholders and names states', () => {
		expect(isPlaceholderDomain('web.localhost')).toBe(true);
		expect(isPlaceholderDomain('web.example.com')).toBe(false);
		expect(dnsHeadline('missing')).toBe('No DNS record yet');
		expect(automaticAddress('web', 'apps.example.com')).toBe('web.apps.example.com');
		expect(automaticAddress('web', null)).toBeNull();
	});
});

describe('AppDomain', () => {
	beforeEach(() => {
		for (const fn of [...Object.values(api), confirm.confirmAction]) fn.mockReset();
		api.listCertificates.mockResolvedValue([]);
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: ''
		});
		api.checkDomain.mockImplementation(async (d: string) => check(d));
		api.listAppDomains.mockResolvedValue([row('app.example.com', 'primary')]);
	});

	it('shows the exact record to create when DNS points elsewhere, and re-checks', async () => {
		const user = userEvent.setup();
		render(AppDomainCard, { props: { app } });

		expect(await screen.findByText('DNS points somewhere else')).toBeInTheDocument();
		expect(screen.getByText('203.0.113.10')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /copy value 203.0.113.10/i })).toBeInTheDocument();
		expect(
			screen.getByText(/create this record at your dns provider for example.com/i)
		).toBeInTheDocument();

		api.checkDomain.mockResolvedValue(
			check('app.example.com', {
				status: 'ok',
				message: 'app.example.com points at this server.',
				resolved: ['203.0.113.10']
			})
		);
		await user.click(screen.getByRole('button', { name: /check again/i }));
		expect(await screen.findByText('DNS points at this server')).toBeInTheDocument();
		expect(api.checkDomain).toHaveBeenCalledTimes(2);
	});

	it('lists every domain with its kind, redirect target and status', async () => {
		api.listAppDomains.mockResolvedValue([
			row('app.example.com', 'primary'),
			row('web.apps.example.com', 'alias', { automatic: true }),
			row('www.app.example.com', 'redirect', { redirect_to: 'app.example.com', redirect_code: 301 })
		]);
		api.listCertificates.mockResolvedValue([
			{ domain: '*.apps.example.com', status: 'active', cert_type: 'acme' }
		]);
		render(AppDomainCard, { props: { app } });

		const rows = await screen.findAllByTestId('domain-row');
		expect(rows).toHaveLength(3);
		expect(within(rows[0]).getByText('Primary')).toBeInTheDocument();
		expect(within(rows[1]).getByText('Automatic')).toBeInTheDocument();
		expect(within(rows[1]).getByText('HTTPS · wildcard')).toBeInTheDocument();
		expect(
			within(rows[2]).getByText(/sends visitors to app.example.com \(301\), keeping the path/i)
		).toBeInTheDocument();
		// The primary can't be made primary or redirected; the others can.
		expect(within(rows[0]).queryByRole('button', { name: /make .* primary/i })).toBeNull();
		expect(
			within(rows[1]).getByRole('button', { name: 'Make web.apps.example.com the primary domain' })
		).toBeInTheDocument();
	});

	it('adds a domain with its www counterpart redirecting to it', async () => {
		const onChanged = vi.fn();
		api.addWwwPair.mockResolvedValue([
			row('app.example.com', 'primary'),
			row('shop.example.com', 'alias'),
			row('www.shop.example.com', 'redirect', { redirect_to: 'shop.example.com' })
		]);
		const user = userEvent.setup();
		render(AppDomainCard, { props: { app, onChanged } });

		await user.click(await screen.findByRole('button', { name: /add domain/i }));
		await user.type(screen.getByLabelText(/^domain$/i), 'not a domain');
		await user.click(screen.getByRole('button', { name: /^add domain$/i }));
		expect(screen.getByRole('alert')).toHaveTextContent(/isn't a valid domain/);
		expect(api.addAppDomain).not.toHaveBeenCalled();

		await user.clear(screen.getByLabelText(/^domain$/i));
		await user.type(screen.getByLabelText(/^domain$/i), 'Shop.Example.com.');
		await user.click(
			screen.getByRole('checkbox', { name: /also add www.shop.example.com, redirecting here/i })
		);
		await user.click(screen.getByRole('button', { name: /^add domain$/i }));
		// The app already has a custom primary, so the new name stays an alias.
		expect(api.addWwwPair).toHaveBeenCalledWith('app_1', 'shop.example.com', 'apex', false);
		expect(await screen.findAllByTestId('domain-row')).toHaveLength(3);
		expect(onChanged).toHaveBeenCalled();
	});

	it('makes a domain primary and removes one after confirmation', async () => {
		api.listAppDomains.mockResolvedValue([
			row('app.example.com', 'primary'),
			row('shop.example.com', 'alias')
		]);
		api.updateAppDomain.mockResolvedValue([
			row('shop.example.com', 'primary'),
			row('app.example.com', 'alias')
		]);
		const user = userEvent.setup();
		render(AppDomainCard, { props: { app } });

		await user.click(
			await screen.findByRole('button', { name: 'Make shop.example.com the primary domain' })
		);
		expect(api.updateAppDomain).toHaveBeenCalledWith('app_1', 'shop.example.com', {
			kind: 'primary'
		});

		confirm.confirmAction.mockImplementation(async (opts: { onconfirm: () => unknown }) => {
			await opts.onconfirm();
			return true;
		});
		api.removeAppDomain.mockResolvedValue([row('shop.example.com', 'primary')]);
		await user.click(await screen.findByRole('button', { name: 'Remove app.example.com' }));
		expect(confirm.confirmAction).toHaveBeenCalledWith(
			expect.objectContaining({ title: 'Remove app.example.com?', destructive: true })
		);
		expect(api.removeAppDomain).toHaveBeenCalledWith('app_1', 'app.example.com');
		expect(await screen.findAllByTestId('domain-row')).toHaveLength(1);
	});

	it('explains the automatic address and custom domain choice without a domain', async () => {
		api.listAppDomains.mockResolvedValue([]);
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: '',
			apps_domain: 'example-apps.com'
		});
		api.addAppDomain.mockResolvedValue([row('shop.example.com', 'primary')]);
		const user = userEvent.setup();
		render(AppDomainCard, { props: { app: { ...app, domain: null } } });

		expect(await screen.findByText('web.example-apps.com')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /add a domain/i }));
		await user.type(screen.getByLabelText(/^domain$/i), 'shop.example.com');
		await user.click(screen.getByRole('button', { name: /^add domain$/i }));
		expect(api.addAppDomain).toHaveBeenCalledWith('app_1', { domain: 'shop.example.com' });
	});

	it('does not check placeholder domains', async () => {
		api.listAppDomains.mockResolvedValue([row('web.localhost', 'primary')]);
		render(AppDomainCard, { props: { app: { ...app, domain: 'web.localhost' } } });
		expect(await screen.findByText(/local placeholder/i)).toBeInTheDocument();
		expect(api.checkDomain).not.toHaveBeenCalled();
	});

	it('warns when the domain is on the same site as the dashboard', async () => {
		api.listAppDomains.mockResolvedValue([
			row('app.example.com', 'primary', {
				warning: 'app.example.com is on the same site as your dashboard (dash.example.com).'
			})
		]);
		render(AppDomainCard, { props: { app } });
		expect(await screen.findByText('Same site as your dashboard')).toBeInTheDocument();
	});

	it('never offers the dashboard domain as the automatic address', async () => {
		api.listAppDomains.mockResolvedValue([]);
		render(AppDomainCard, { props: { app: { ...app, domain: null } } });
		expect(await screen.findByText(/No apps domain is set/)).toBeInTheDocument();
		expect(screen.queryByText(/web\.dash\.example\.com/)).not.toBeInTheDocument();
	});
});
