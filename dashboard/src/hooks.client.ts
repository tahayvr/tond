import type { ClientInit, HandleClientError } from '@sveltejs/kit';
import { ApiError } from '$lib/api';

// Start the MSW mock service worker in dev when VITE_MOCK_API=true. This runs in the
// `init` hook, which SvelteKit awaits before any `load` function, so page loaders' API
// calls are intercepted even on a hard refresh.
//
// Production builds never include mocks: `import.meta.env.DEV` is statically false
// there, so the dynamic import below is dropped from the bundle, and the worker script
// lives in dev/ (served only by the dev server), not static/.
export const init: ClientInit = async () => {
	if (import.meta.env.DEV && import.meta.env.VITE_MOCK_API === 'true') {
		const { worker } = await import('./mocks/browser');
		await worker.start({
			serviceWorker: { url: '/mockServiceWorker.js' },
			onUnhandledRequest: 'bypass'
		});
	}
};

// Keep API failure details for +error.svelte instead of a generic "Internal Error".
export const handleError: HandleClientError = ({ error, status, message }) => {
	if (error instanceof ApiError) {
		return { message: error.message, status: error.status };
	}
	return { message: status === 404 ? 'This page does not exist.' : message, status };
};
