<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { checkAuth, getStatus } from '$lib/auth.svelte';
	import { authRedirect, isAuthRoute as isAuthRoutePath, showsAppShell } from '$lib/auth-routes';
	import {
		getFeedOpen,
		getPaletteOpen,
		getShortcutsOpen,
		getTerminalOpen,
		setFeedOpen,
		setPaletteOpen,
		setShortcutsOpen,
		setTerminalOpen
	} from '$lib/ui-context.svelte';
	import { ModeWatcher } from 'mode-watcher';
	import Header from '$lib/components/header/header.svelte';
	import Footer from '$lib/components/footer/footer.svelte';
	import TerminalPane from '$lib/components/terminal/terminal-pane.svelte';
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import FeedPanel from '$lib/components/feed-panel/feed-panel.svelte';
	import { Toaster } from '$lib/components/ui/sonner';
	import { listAlertRules } from '$lib/api';
	import { toast } from 'svelte-sonner';
	import { SvelteMap } from 'svelte/reactivity';
	import { registerDashboardShortcuts } from '$lib/dashboard-shortcuts';
	import { pollWhileVisible } from '$lib/polling';
	import { untrack } from 'svelte';
	import favicon from '$lib/assets/favicon.svg';
	import NavProgress from '$lib/components/nav/nav-progress.svelte';
	import ConfirmHost from '$lib/components/dialogs/confirm-host.svelte';
	import CommandPalette from '$lib/components/command/command-palette.svelte';
	import ShortcutsDialog from '$lib/components/command/shortcuts-dialog.svelte';

	import './layout.css';

	let { children } = $props();

	// Check auth on mount. MSW (mock API) is already running by now: it is started
	// in the `init` hook in hooks.client.ts.
	$effect(() => {
		if (typeof window === 'undefined') return;
		checkAuth();
	});

	// Redirect based on auth status + current path. Signed-out users keep the
	// page they were on as ?next= so they return to it after signing in.
	$effect(() => {
		if (typeof window === 'undefined') return;

		const target = authRedirect(getStatus(), page.url);
		if (target !== null) goto(target, { replaceState: true });
	});

	// True when we know what to show: auth boot completed.
	let ready = $derived(getStatus() !== 'loading');
	let isAuthRoute = $derived(isAuthRoutePath(page.url.pathname));
	// Never render the dashboard shell for a signed-out user, even for the
	// moment before the redirect above lands.
	let showShell = $derived(showsAppShell(getStatus(), page.url.pathname));
	let terminalOpen = $derived(getTerminalOpen());
	let showBootShell = $state(false);

	$effect(() => {
		if (getStatus() !== 'loading') {
			showBootShell = false;
			return;
		}

		const timer = window.setTimeout(() => {
			showBootShell = true;
		}, 350);

		return () => {
			window.clearTimeout(timer);
			showBootShell = false;
		};
	});

	// ── Alert firing notifications (60 s polling) ────────────────────────────
	// Keeps a snapshot of last_fired_at per rule. When a value changes we show
	// a toast so the user is notified even if they are not on the alerts page.
	// Only runs while authenticated.

	const ALERT_POLL_MS = 60_000;
	const alertSnapshot = new SvelteMap<string, string | null>();

	function metricLabel(metric: string | null): string {
		if (metric === 'cpu') return 'CPU usage threshold exceeded';
		if (metric === 'memory') return 'Memory usage threshold exceeded';
		if (metric === 'app_down') return 'App is down';
		if (metric === 'disk_usage') return 'Disk usage threshold exceeded';
		return 'Alert condition triggered';
	}

	$effect(() => {
		if (getStatus() !== 'authenticated') return;

		async function poll() {
			try {
				const rules = await listAlertRules();

				for (const rule of rules) {
					const prev = alertSnapshot.get(rule.id);
					// prev === undefined means it's the first poll — just seed the snapshot.
					// prev !== undefined and different means it fired since last check.
					if (prev !== undefined && rule.last_fired_at !== prev && rule.last_fired_at !== null) {
						toast.warning(`Alert fired: ${rule.name}`, {
							description: metricLabel(rule.metric)
						});
					}
				}

				// Rebuild snapshot from current rules
				alertSnapshot.clear();
				for (const rule of rules) alertSnapshot.set(rule.id, rule.last_fired_at);
			} catch {
				// Polling failures are silent — don't interrupt the user
			}
		}

		// Seed the snapshot immediately, then poll while the tab is visible.
		return pollWhileVisible(() => untrack(poll), ALERT_POLL_MS);
	});

	$effect(() => {
		if (typeof window === 'undefined' || isAuthRoute) return;

		return registerDashboardShortcuts(() => ({ terminalOpen }), {
			setTerminalOpen,
			openPalette: () => setPaletteOpen(true),
			openShortcuts: () => setShortcutsOpen(true),
			navigate: (href) => goto(href)
		});
	});
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher />
<Toaster richColors position="top-right" />
<NavProgress />
<ConfirmHost />
{#if isAuthRoute}
	<div class="min-h-screen bg-background text-foreground">
		{@render children()}
	</div>
{:else if !ready}
	{#if showBootShell}
		<div class="min-h-screen bg-background text-foreground">
			<div class="mx-auto flex min-h-screen w-full max-w-4xl items-center justify-center px-6">
				<div class="w-full max-w-2xl rounded-lg border border-border bg-card p-8 shadow-sm">
					<h1 class="text-3xl font-extrabold">Checking server status</h1>
					<p class="mt-3 max-w-xl text-sm text-muted-foreground">
						Authenticating your session and loading the control plane. The first load right after
						install can take a moment.
					</p>

					<div class="mt-8 grid gap-3 sm:grid-cols-3">
						<div class="rounded-md border border-border bg-background px-4 py-4">
							<p class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
								Auth
							</p>
							<div class="mt-3 flex items-center gap-3">
								<div class="h-2.5 w-2.5 animate-pulse rounded-full bg-primary"></div>
								<span class="text-sm text-muted-foreground">Checking login state</span>
							</div>
						</div>

						<div class="rounded-md border border-border bg-background px-4 py-4">
							<p class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
								Routes
							</p>
							<div class="mt-3 flex items-center gap-3">
								<div class="h-2.5 w-2.5 animate-pulse rounded-full bg-primary"></div>
								<span class="text-sm text-muted-foreground">Preparing dashboard</span>
							</div>
						</div>

						<div class="rounded-md border border-border bg-background px-4 py-4">
							<p class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
								Install
							</p>
							<div class="mt-3 flex items-center gap-3">
								<div class="h-2.5 w-2.5 animate-pulse rounded-full bg-primary"></div>
								<span class="text-sm text-muted-foreground">Waiting for services</span>
							</div>
						</div>
					</div>
				</div>
			</div>
		</div>
	{:else}
		<div class="min-h-screen bg-background"></div>
	{/if}
{:else if !showShell}
	<!-- Signed out on a protected page: the redirect to /login is in flight. -->
	<div class="min-h-screen bg-background"></div>
{:else}
	<Tooltip.Provider>
		<a
			href="#main-content"
			class="sr-only z-50 rounded-sm bg-background px-3 py-2 text-sm focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
			>Skip to content</a
		>
		<!-- The provider wraps the whole shell so the footer can toggle the feed (a sheet on phones). -->
		<Sidebar.Provider
			open={getFeedOpen()}
			onOpenChange={setFeedOpen}
			class="flex h-dvh flex-col bg-background"
		>
			<!-- header: non-flexing top bar -->
			<div class="flex-none">
				<Header />
			</div>

			<!-- main: takes remaining height, the feed sidebar is contained here -->
			<div class="relative flex min-h-0 flex-1 overflow-hidden">
				{#if terminalOpen}
					<div class="min-w-0 flex-1 overflow-hidden">
						<TerminalPane />
					</div>
				{:else}
					<main id="main-content" tabindex="-1" class="min-w-0 flex-1 overflow-auto p-4 sm:p-6">
						{@render children()}
					</main>
				{/if}

				{#if !terminalOpen}
					<FeedPanel />
				{/if}
			</div>

			<!-- footer: stays at the bottom of the window -->
			<div class="flex-none">
				<Footer />
			</div>
		</Sidebar.Provider>
		<CommandPalette
			bind:open={() => getPaletteOpen(), setPaletteOpen}
			onShowShortcuts={() => setShortcutsOpen(true)}
		/>
		<ShortcutsDialog bind:open={() => getShortcutsOpen(), setShortcutsOpen} />
	</Tooltip.Provider>
{/if}
