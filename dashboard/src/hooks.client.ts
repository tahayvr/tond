import type { ClientInit } from '@sveltejs/kit';

// Start the MSW mock service worker in dev when VITE_MOCK_API=true. This runs in the
// `init` hook, which SvelteKit awaits before any `load` function, so page loaders' API
// calls are intercepted even on a hard refresh. In prod this is a no-op (tree-shaken out).
export const init: ClientInit = async () => {
	if (import.meta.env.DEV && import.meta.env.VITE_MOCK_API === 'true') {
		const { worker } = await import('./mocks/browser');
		await worker.start({
			serviceWorker: { url: '/mockServiceWorker.js' },
			onUnhandledRequest: 'bypass'
		});
	}
};
