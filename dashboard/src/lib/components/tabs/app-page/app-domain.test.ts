import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { App, DomainCheck } from '$lib/api';
import AppDomain, { automaticAddress, dnsHeadline, isPlaceholderDomain } from './app-domain.svelte';

const api = vi.hoisted(() => ({
	checkDomain: vi.fn(),
	getSettings: vi.fn(),
	listCertificates: vi.fn(),
	retryCertificate: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const app = {
	id: 'app_1',
	name: 'web',
	domain: 'app.example.com'
} as App;

function check(overrides: Partial<DomainCheck['dns']> = {}): DomainCheck {
	return {
		domain: 'app.example.com',
		zone: 'example.com',
		apps_domain: 'apps.example.com',
		under_apps_domain: false,
		records: [{ type: 'A', name: 'app', host: 'app.example.com', value: '203.0.113.10' }],
		wildcard_records: [],
		server_ips: ['203.0.113.10'],
		dns: {
			status: 'mismatch',
			resolved: ['198.51.100.1'],
			expected: ['203.0.113.10'],
			stray: [],
			message: 'DNS for app.example.com points to 198.51.100.1, expected 203.0.113.10.',
			...overrides
		},
		certificate: null,
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
		for (const fn of Object.values(api)) fn.mockReset();
		api.listCertificates.mockResolvedValue([]);
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: ''
		});
	});

	it('shows the exact record to create when DNS points elsewhere, and re-checks', async () => {
		api.checkDomain.mockResolvedValue(check());
		const user = userEvent.setup();
		render(AppDomain, { props: { app, onUpdate: vi.fn() } });

		expect(await screen.findByText('DNS points somewhere else')).toBeInTheDocument();
		expect(screen.getByText('203.0.113.10')).toBeInTheDocument();
		expect(screen.getByRole('button', { name: /copy value 203.0.113.10/i })).toBeInTheDocument();
		expect(
			screen.getByText(/create this record at your dns provider for example.com/i)
		).toBeInTheDocument();

		api.checkDomain.mockResolvedValue(
			check({
				status: 'ok',
				message: 'app.example.com points at this server.',
				resolved: ['203.0.113.10']
			})
		);
		await user.click(screen.getByRole('button', { name: /check again/i }));
		expect(await screen.findByText('DNS points at this server')).toBeInTheDocument();
		expect(api.checkDomain).toHaveBeenCalledTimes(2);
	});

	it('explains the automatic address and custom domain choice without a domain', async () => {
		const onUpdate = vi.fn().mockResolvedValue(undefined);
		api.getSettings.mockResolvedValue({
			server_name: 'Tond',
			dashboard_domain: 'dash.example.com',
			preview_domain: '',
			apps_domain: 'example-apps.com'
		});
		const user = userEvent.setup();
		render(AppDomain, { props: { app: { ...app, domain: null }, onUpdate } });

		expect(await screen.findByText('web.example-apps.com')).toBeInTheDocument();
		await user.click(screen.getByRole('button', { name: /add a domain/i }));
		await user.type(screen.getByLabelText(/custom domain/i), 'not a domain');
		await user.click(screen.getByRole('button', { name: /save domain/i }));
		expect(screen.getByRole('alert')).toHaveTextContent(/isn't a valid domain/);
		expect(onUpdate).not.toHaveBeenCalled();

		await user.clear(screen.getByLabelText(/custom domain/i));
		await user.type(screen.getByLabelText(/custom domain/i), 'Shop.Example.com.');
		await user.click(screen.getByRole('button', { name: /save domain/i }));
		expect(onUpdate).toHaveBeenCalledWith('shop.example.com');
		expect(api.checkDomain).not.toHaveBeenCalled();
	});

	it('does not check placeholder domains', () => {
		render(AppDomain, { props: { app: { ...app, domain: 'web.localhost' }, onUpdate: vi.fn() } });
		expect(screen.getByText(/local placeholder/i)).toBeInTheDocument();
		expect(api.checkDomain).not.toHaveBeenCalled();
	});

	it('warns when the domain is on the same site as the dashboard', async () => {
		api.checkDomain.mockResolvedValue({
			...check({ status: 'ok', resolved: ['203.0.113.10'] }),
			warning: 'app.example.com is on the same site as your dashboard (dash.example.com).'
		});
		render(AppDomain, { props: { app, onUpdate: vi.fn() } });
		expect(await screen.findByText('Same site as your dashboard')).toBeInTheDocument();
	});

	it('never offers the dashboard domain as the automatic address', async () => {
		render(AppDomain, { props: { app: { ...app, domain: null }, onUpdate: vi.fn() } });
		expect(await screen.findByText(/No apps domain is set/)).toBeInTheDocument();
		expect(screen.queryByText(/web\.dash\.example\.com/)).not.toBeInTheDocument();
	});
});
