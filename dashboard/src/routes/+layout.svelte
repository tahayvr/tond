<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { checkAuth, getStatus } from '$lib/auth.svelte';
	import {
		getFeedOpen,
		getTerminalOpen,
		setFeedOpen,
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
	import favicon from '$lib/assets/favicon.svg';

	import './layout.css';

	let { children } = $props();

	// Check auth on mount. MSW (mock API) is already running by now: it is started
	// in the `init` hook in hooks.client.ts.
	$effect(() => {
		if (typeof window === 'undefined') return;
		checkAuth();
	});

	// Redirect based on auth status + current path
	$effect(() => {
		if (typeof window === 'undefined') return;

		const status = getStatus();
		const path = page.url.pathname;

		if (status === 'loading') return;

		if (status === 'setup' && path !== '/setup') {
			goto('/setup');
		} else if (status === 'login' && path !== '/login') {
			goto('/login');
		} else if (status === 'authenticated' && (path === '/login' || path === '/setup')) {
			goto('/');
		}
	});

	// True when we know what to show: auth boot completed.
	let ready = $derived(getStatus() !== 'loading');
	let isAuthRoute = $derived(page.url.pathname === '/setup' || page.url.pathname === '/login');
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

		let stopped = false;

		async function poll() {
			if (stopped) return;
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

			if (!stopped) {
				setTimeout(poll, ALERT_POLL_MS);
			}
		}

		// Seed snapshot immediately, then start polling
		poll();

		return () => {
			stopped = true;
		};
	});

	$effect(() => {
		if (typeof window === 'undefined' || isAuthRoute) return;

		return registerDashboardShortcuts(() => ({ terminalOpen }), { setTerminalOpen });
	});
</script>

<svelte:head>
	<title>Tond</title>
	<link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher />
<Toaster richColors position="top-right" />
{#if isAuthRoute}
	<div class="min-h-screen bg-background text-foreground">
		{@render children()}
	</div>
{:else if !ready}
	{#if showBootShell}
		<div class="min-h-screen bg-background text-foreground">
			<div class="mx-auto flex min-h-screen w-full max-w-4xl items-center justify-center px-6">
				<div class="w-full max-w-2xl rounded-lg border border-border bg-card p-8 shadow-sm">
					<p class="font-mono text-[10px] tracking-[0.35em] text-muted-foreground/70 uppercase">
						TOND_BOOT
					</p>
					<h1 class="mt-3 text-3xl font-extrabold uppercase">Checking server status</h1>
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
{:else}
	<Tooltip.Provider>
		<div class="flex h-screen flex-col bg-background">
			<!-- header: non-flexing top bar -->
			<div class="flex-none">
				<Header />
			</div>

			<!-- main: takes remaining height, sidebar is contained here -->
			<Sidebar.Provider
				open={getFeedOpen()}
				onOpenChange={setFeedOpen}
				class="relative flex-1 overflow-hidden"
			>
				{#if terminalOpen}
					<div class="min-w-0 flex-1 overflow-hidden">
						<TerminalPane />
					</div>
				{:else}
					<main class="min-w-0 flex-1 overflow-auto p-6">
						{@render children()}
					</main>
				{/if}

				{#if !terminalOpen}
					<FeedPanel />
				{/if}
			</Sidebar.Provider>

			<!-- footer: stays at the bottom of the window -->
			<div class="flex-none">
				<Footer />
			</div>
		</div>
	</Tooltip.Provider>
{/if}
