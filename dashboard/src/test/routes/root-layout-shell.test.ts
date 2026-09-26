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
	getStatus: () => state.status,
	can: () => false
}));

vi.mock('$lib/ui-context.svelte', () => ({
	getFeedOpen: () => false,
	getTerminalOpen: () => false,
	getPaletteOpen: () => false,
	getShortcutsOpen: () => false,
	setFeedOpen: vi.fn(),
	setTerminalOpen: vi.fn(),
	setPaletteOpen: vi.fn(),
	setShortcutsOpen: vi.fn()
}));

vi.mock('$lib/components/command/command-palette.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/command/shortcuts-dialog.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
}));

vi.mock('$lib/components/nav/nav-progress.svelte', async () => ({
	default: (await import('../mocks/EmptyComponent.svelte')).default
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

	it('renders /login/2fa bare, without redirecting a signed-out user back to /login', async () => {
		const { goto } = await import('$app/navigation');
		state.page.url = new URL('http://localhost/login/2fa');
		state.status = 'login';

		render(RootLayout, {
			props: {
				children: createRawSnippet(() => ({
					render: () => '<div>two factor page</div>'
				}))
			}
		});

		expect(screen.getByText('two factor page')).toBeInTheDocument();
		expect(goto).not.toHaveBeenCalled();
	});

	it('does not render the dashboard for a signed-out user and redirects with next', async () => {
		const { goto } = await import('$app/navigation');
		state.page.url = new URL('http://localhost/apps/app_1');
		state.status = 'login';

		render(RootLayout, {
			props: {
				children: createRawSnippet(() => ({
					render: () => '<div>dashboard child content</div>'
				}))
			}
		});

		expect(screen.queryByText('dashboard child content')).not.toBeInTheDocument();
		expect(goto).toHaveBeenCalledWith('/login?next=%2Fapps%2Fapp_1', { replaceState: true });
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
