import '@testing-library/jest-dom/vitest';

import { cleanup } from '@testing-library/svelte';
import { afterAll, afterEach, beforeAll, vi } from 'vitest';

import { server } from './server';

const originalFetch = globalThis.fetch.bind(globalThis);

beforeAll(() => {
	server.listen({ onUnhandledRequest: 'error' });

	globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
		const resolvedInput =
			typeof input === 'string' && input.startsWith('/')
				? new URL(input, 'http://localhost').toString()
				: input instanceof URL && input.pathname.startsWith('/') && input.origin === 'null'
					? new URL(input.pathname + input.search, 'http://localhost').toString()
					: input;

		return originalFetch(resolvedInput as RequestInfo | URL, init);
	}) as typeof globalThis.fetch;

	Object.defineProperty(window, 'matchMedia', {
		writable: true,
		value: vi.fn().mockImplementation((query: string) => ({
			matches: false,
			media: query,
			onchange: null,
			addEventListener: vi.fn(),
			removeEventListener: vi.fn(),
			addListener: vi.fn(),
			removeListener: vi.fn(),
			dispatchEvent: vi.fn()
		}))
	});

	Object.defineProperty(window, 'CSS', {
		writable: true,
		value: {
			supports: vi.fn().mockReturnValue(false)
		}
	});

	Object.defineProperty(navigator, 'clipboard', {
		configurable: true,
		value: {
			writeText: vi.fn()
		}
	});

	Object.defineProperty(window, 'scrollTo', {
		writable: true,
		value: vi.fn()
	});

	Object.defineProperty(document, 'elementFromPoint', {
		writable: true,
		value: vi.fn().mockReturnValue(null)
	});

	class ResizeObserverMock {
		observe() {}
		unobserve() {}
		disconnect() {}
	}

	vi.stubGlobal('ResizeObserver', ResizeObserverMock);
});

afterEach(() => {
	cleanup();
	// A dialog closed mid-animation can leave bits-ui's scroll/pointer lock on <body>.
	document.body.removeAttribute('style');
	server.resetHandlers();
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
	vi.useRealTimers();
	vi.resetModules();
	window.localStorage.clear();
	window.sessionStorage.clear();
});

afterAll(() => {
	globalThis.fetch = originalFetch;
	server.close();
});
