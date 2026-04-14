import { render, screen } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const state = vi.hoisted(() => ({
	page: { url: new URL('http://localhost/projects') },
	navigating: { type: 'link' as null | string },
	status: 'authenticated' as 'loading' | 'setup' | 'login' | 'authenticated'
}));

vi.mock('$app/navigation', () => ({
	goto: vi.fn()
}));

vi.mock('$app/state', () => ({
	page: state.page,
	navigating: state.navigating
}));

vi.mock('$lib/auth.svelte', () => ({
	checkAuth: vi.fn().mockResolvedValue(undefined),
	getStatus: () => state.status
}));

vi.mock('$lib/ui-context.svelte', () => ({
	getFeedOpen: () => false,
	getTerminalOpen: () => false,
	setFeedOpen: vi.fn(),
	setTerminalOpen: vi.fn()
}));

vi.mock('$lib/api', () => ({
	listAlertRules: vi.fn().mockResolvedValue([])
}));

vi.mock('svelte-sonner', () => ({
	toast: { warning: vi.fn() }
}));

vi.mock('$lib/dashboard-shortcuts', () => ({
	registerDashboardShortcuts: vi.fn(() => () => {})
}));

vi.mock('mode-watcher', async () => ({
	ModeWatcher: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/header/header.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/footer/footer.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/terminal/terminal-pane.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/feed-panel/feed-panel.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/ui/sonner', async () => ({
	Toaster: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/ui/sidebar/index.js', async () => ({
	Provider: (await import('../mocks/SnippetWrapper.svelte')).default
}));

vi.mock('$lib/components/ui/tooltip/index.js', async () => ({
	Provider: (await import('../mocks/SnippetWrapper.svelte')).default
}));

import RootLayout from '../../routes/+layout.svelte';

describe('root layout shell', () => {
	beforeEach(() => {
		vi.stubEnv('VITE_MOCK_API', 'false');
		vi.stubEnv('VITE_DEV_AUTH_BYPASS', 'false');
		state.page.url = new URL('http://localhost/projects');
		state.navigating.type = 'link';
		state.status = 'authenticated';
	});

	it('keeps the authenticated page content visible during client-side navigation', () => {
		render(RootLayout, {
			props: {
				children: createRawSnippet(() => ({
					render: () => '<div>dashboard child content</div>'
				}))
			}
		});

		expect(screen.getByText('dashboard child content')).toBeInTheDocument();
		expect(screen.queryByText(/checking server status/i)).not.toBeInTheDocument();
	});

	it('shows the boot shell while auth status is still loading', async () => {
		state.navigating.type = null;
		state.status = 'loading';
		vi.useFakeTimers();

		render(RootLayout, {
			props: {
				children: createRawSnippet(() => ({
					render: () => '<div>dashboard child content</div>'
				}))
			}
		});

		await vi.advanceTimersByTimeAsync(351);

		expect(screen.getByText(/checking server status/i)).toBeInTheDocument();
		expect(screen.queryByText('dashboard child content')).not.toBeInTheDocument();
	});
});
