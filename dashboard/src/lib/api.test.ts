import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
	ApiError,
	CSRF_HEADER,
	SERVER_UNAVAILABLE_MESSAGE,
	SESSION_EXPIRED_MESSAGE,
	changePassword,
	createTerminalTicket,
	deployApp,
	listApps,
	login,
	parseRetryAfter,
	setUnauthorizedHandler,
	setupAdmin
} from './api';

// The shared test setup wraps fetch before MSW patches it, so these tests stub
// fetch directly with the exact responses they need.
function stubFetch(response: Response | Error) {
	const fetchMock = vi.fn(async () => {
		if (response instanceof Error) throw response;
		return response.clone();
	});
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

function json(body: unknown, init: ResponseInit = {}) {
	return new Response(JSON.stringify(body), {
		...init,
		headers: { 'Content-Type': 'application/json', ...(init.headers ?? {}) }
	});
}

async function rejection(promise: Promise<unknown>): Promise<ApiError> {
	try {
		await promise;
	} catch (e) {
		expect(e).toBeInstanceOf(ApiError);
		return e as ApiError;
	}
	throw new Error('expected the request to fail');
}

afterEach(() => {
	setUnauthorizedHandler(null);
	vi.unstubAllGlobals();
});

describe('request()', () => {
	it('returns data for a successful response', async () => {
		stubFetch(json({ success: true, data: [{ id: 'app_1' }], error: null }));

		await expect(listApps()).resolves.toEqual([{ id: 'app_1' }]);
	});

	it('treats a 401 on a normal endpoint as an expired session', async () => {
		const onUnauthorized = vi.fn();
		setUnauthorizedHandler(onUnauthorized);
		stubFetch(json({ success: false, data: null, error: 'Unauthorized' }, { status: 401 }));

		const error = await rejection(listApps());

		expect(error.status).toBe(401);
		expect(error.message).toBe(SESSION_EXPIRED_MESSAGE);
		expect(onUnauthorized).toHaveBeenCalledOnce();
	});

	it('does not sign the user out for a wrong password or code', async () => {
		const onUnauthorized = vi.fn();
		setUnauthorizedHandler(onUnauthorized);

		stubFetch(
			json({ success: false, data: null, error: 'Invalid email or password' }, { status: 401 })
		);
		expect((await rejection(login('a@b.c', 'nope'))).message).toBe('Invalid email or password');

		stubFetch(
			json({ success: false, data: null, error: 'Current password is incorrect' }, { status: 401 })
		);
		expect((await rejection(changePassword('wrong', 'new-password'))).message).toBe(
			'Current password is incorrect'
		);

		expect(onUnauthorized).not.toHaveBeenCalled();
	});

	it('reports a proxy HTML error page as server unavailable', async () => {
		stubFetch(
			new Response('<html><body>502 Bad Gateway</body></html>', {
				status: 502,
				headers: { 'Content-Type': 'text/html' }
			})
		);

		const error = await rejection(listApps());

		expect(error.status).toBe(502);
		expect(error.message).toBe(SERVER_UNAVAILABLE_MESSAGE);
		expect(error.message).not.toMatch(/unexpected token/i);
	});

	it('reports a network failure as server unavailable', async () => {
		stubFetch(new TypeError('fetch failed'));

		const error = await rejection(listApps());

		expect(error.status).toBe(0);
		expect(error.message).toBe(SERVER_UNAVAILABLE_MESSAGE);
	});

	it('honours Retry-After on 429', async () => {
		stubFetch(
			json(
				{ success: false, data: null, error: 'Rate limit exceeded. Try again later.' },
				{ status: 429, headers: { 'Retry-After': '42' } }
			)
		);

		const error = await rejection(login('a@b.c', 'pw'));

		expect(error.status).toBe(429);
		expect(error.retryAfter).toBe(42);
		expect(error.message).toBe('Too many attempts, try again in 42 s.');
	});

	it('falls back to a generic 429 message without Retry-After', async () => {
		stubFetch(json({ success: false, data: null, error: 'x' }, { status: 429 }));

		expect((await rejection(listApps())).message).toBe('Too many attempts, try again in a moment.');
	});

	it('uses the server error message for other failures', async () => {
		stubFetch(
			json({ success: false, data: null, error: 'Docker error: daemon down' }, { status: 500 })
		);

		const error = await rejection(listApps());

		expect(error.status).toBe(500);
		expect(error.message).toBe('Docker error: daemon down');
	});
});

describe('parseRetryAfter', () => {
	it('parses seconds and HTTP dates', () => {
		const now = Date.parse('2026-01-01T00:00:00Z');
		expect(parseRetryAfter('7', now)).toBe(7);
		expect(parseRetryAfter('Thu, 01 Jan 2026 00:00:30 GMT', now)).toBe(30);
		expect(parseRetryAfter('garbage', now)).toBeNull();
		expect(parseRetryAfter(null, now)).toBeNull();
	});
});

const fetchMock = vi.fn();

function lastRequest(): { url: string; init: RequestInit; headers: Headers } {
	const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit];
	return { url, init, headers: new Headers(init.headers) };
}

describe('api request helper (headers and bodies)', () => {
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
