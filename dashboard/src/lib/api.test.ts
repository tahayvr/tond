import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { CSRF_HEADER, createTerminalTicket, deployApp, listApps, setupAdmin } from './api';

const fetchMock = vi.fn();

function lastRequest(): { url: string; init: RequestInit; headers: Headers } {
	const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit];
	return { url, init, headers: new Headers(init.headers) };
}

describe('api request helper', () => {
	beforeEach(() => {
		fetchMock.mockReset();
		fetchMock.mockResolvedValue(
			new Response(JSON.stringify({ success: true, data: [], error: null }))
		);
		vi.stubGlobal('fetch', fetchMock);
	});

	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it('sends the CSRF header and same-origin credentials on reads', async () => {
		await listApps();

		const { url, init, headers } = lastRequest();
		expect(url).toBe('/api/apps');
		expect(init.credentials).toBe('same-origin');
		expect(headers.get(CSRF_HEADER)).toBe('1');
		expect(headers.get('Content-Type')).toBe('application/json');
	});

	it('sends the CSRF header on body-less mutations', async () => {
		await deployApp('app_1');

		const { init, headers } = lastRequest();
		expect(init.method).toBe('POST');
		expect(headers.get(CSRF_HEADER)).toBe('1');
	});

	it('includes the one-time token when creating the admin account', async () => {
		await setupAdmin('tok', 'admin@example.com', 'StrongPassword1!');

		const { init } = lastRequest();
		expect(JSON.parse(init.body as string)).toEqual({
			token: 'tok',
			email: 'admin@example.com',
			password: 'StrongPassword1!'
		});
	});

	it('requests terminal tickets with step-up credentials', async () => {
		fetchMock.mockResolvedValue(
			new Response(
				JSON.stringify({ success: true, data: { ticket: 't', expires_in: 30 }, error: null })
			)
		);

		const ticket = await createTerminalTicket(
			{ id: 'server', kind: 'server' },
			{ password: 'pw', code: '123456' }
		);

		expect(ticket.ticket).toBe('t');
		const { url, init } = lastRequest();
		expect(url).toBe('/api/terminal/tickets');
		expect(JSON.parse(init.body as string)).toEqual({
			target_kind: 'server',
			target_id: 'server',
			password: 'pw',
			code: '123456'
		});
	});

	it('turns a non-JSON error response into a readable error', async () => {
		fetchMock.mockResolvedValue(new Response('Method Not Allowed', { status: 405 }));

		await expect(listApps()).rejects.toThrow('Request failed (HTTP 405)');
	});
});
