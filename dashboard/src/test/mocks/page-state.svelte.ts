/* eslint-disable svelte/prefer-svelte-reactivity -- the URL is replaced, never mutated */
// A reactive stand-in for `page` from $app/state, so tests can "navigate"
// between URL-backed tabs:
//
//   vi.mock('$app/state', async () => ({ page: (await import('../mocks/page-state.svelte')).page }));
//   setPageUrl('http://localhost/apps/app_1?tab=logs'); await tick();

export const page = $state<{ url: URL; params: Record<string, string> }>({
	url: new URL('http://localhost/'),
	params: {}
});

export function setPageUrl(url: string | URL, params?: Record<string, string>) {
	page.url = new URL(url);
	if (params) page.params = params;
}
