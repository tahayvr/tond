import { describe, expect, it } from 'vitest';

import {
	AUTH_ROUTES,
	authRedirect,
	isAuthRoute,
	loginUrl,
	safeNextPath,
	showsAppShell
} from './auth-routes';

const url = (path: string) => new URL(path, 'http://localhost');

describe('AUTH_ROUTES', () => {
	it('includes the 2FA step so it is not bounced back to /login', () => {
		expect(AUTH_ROUTES).toContain('/login/2fa');
		expect(isAuthRoute('/login/2fa')).toBe(true);
		expect(isAuthRoute('/login/2fa/')).toBe(true);
		expect(isAuthRoute('/apps')).toBe(false);
		expect(isAuthRoute('/')).toBe(false);
	});
});

describe('authRedirect', () => {
	it('lets a signed-out user stay on every auth route, including the setup link', () => {
		expect(authRedirect('login', url('/login'))).toBeNull();
		expect(authRedirect('login', url('/login/2fa'))).toBeNull();
		expect(authRedirect('login', url('/setup?token=abc'))).toBeNull();
	});

	it('returns a signed-out admin to a GitHub App completion page with its query', () => {
		const target = authRedirect(
			'login',
			url('/integrations/github/complete?code=c0de&state=s7ate')
		);
		expect(target).toBe(
			'/login?next=%2Fintegrations%2Fgithub%2Fcomplete%3Fcode%3Dc0de%26state%3Ds7ate'
		);
		expect(
			authRedirect(
				'authenticated',
				url(
					`/login?next=${encodeURIComponent('/integrations/github/complete?code=c0de&state=s7ate')}`
				)
			)
		).toBe('/integrations/github/complete?code=c0de&state=s7ate');
	});

	it('sends a signed-out user to /login with the page they were on', () => {
		expect(authRedirect('login', url('/apps/app_1?tab=env'))).toBe(
			'/login?next=%2Fapps%2Fapp_1%3Ftab%3Denv'
		);
		expect(authRedirect('login', url('/'))).toBe('/login');
	});

	it('returns a signed-in user from an auth route to the validated next page', () => {
		expect(authRedirect('authenticated', url('/login?next=%2Fapps%2Fapp_1'))).toBe('/apps/app_1');
		expect(authRedirect('authenticated', url('/login/2fa?next=%2Fcron'))).toBe('/cron');
		expect(authRedirect('authenticated', url('/login?next=https%3A%2F%2Fevil.test'))).toBe('/');
		expect(authRedirect('authenticated', url('/setup'))).toBe('/');
		expect(authRedirect('authenticated', url('/apps'))).toBeNull();
	});

	it('waits while loading', () => {
		expect(authRedirect('loading', url('/apps'))).toBeNull();
	});
});

describe('safeNextPath', () => {
	it('accepts same-origin paths with query and hash', () => {
		expect(safeNextPath('/apps/app_1?tab=logs#bottom')).toBe('/apps/app_1?tab=logs#bottom');
	});

	it.each([
		null,
		'',
		'apps',
		'https://evil.test/',
		'//evil.test/path',
		'/\\evil.test',
		'/\u0000x',
		'javascript:alert(1)',
		'/login',
		'/login/2fa?next=/x',
		'/setup'
	])('rejects %j', (next) => {
		expect(safeNextPath(next)).toBe('/');
	});
});

describe('loginUrl', () => {
	it('omits next for the home page and auth routes', () => {
		expect(loginUrl(url('/'))).toBe('/login');
		expect(loginUrl(url('/login'))).toBe('/login');
		expect(loginUrl(null)).toBe('/login');
	});
});

describe('showsAppShell', () => {
	it('renders the shell only for signed-in users on app routes', () => {
		expect(showsAppShell('authenticated', '/apps')).toBe(true);
		expect(showsAppShell('authenticated', '/login/2fa')).toBe(false);
		expect(showsAppShell('login', '/apps')).toBe(false);
		expect(showsAppShell('login', '/login/2fa')).toBe(false);
		expect(showsAppShell('loading', '/apps')).toBe(false);
	});
});

describe('set-password links', () => {
	it('stay open for signed-out and signed-in visitors, without the app shell', () => {
		expect(authRedirect('login', url('/set-password?token=abc'))).toBeNull();
		expect(authRedirect('authenticated', url('/set-password?token=abc'))).toBeNull();
		expect(showsAppShell('authenticated', '/set-password')).toBe(false);
		expect(safeNextPath('/set-password?token=abc')).toBe('/');
	});
});
