<script lang="ts">
	// Home page tile: traffic over the last 24 hours — totals, the busiest
	// apps and the apps answering with server errors.
	import { untrack } from 'svelte';
	import { getTrafficOverview, type TrafficOverview } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { formatCount, formatMs, formatPercent, formatRps } from '$lib/metrics-format';
	import { appTabHref } from '$lib/tabs';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import GlobeIcon from '~icons/ri/global-line';
	import AlertIcon from '~icons/ri/error-warning-line';
	import CheckIcon from '~icons/ri/checkbox-circle-line';

	let data = $state<TrafficOverview | null>(null);
	let error = $state('');

	async function load() {
		try {
			data = await getTrafficOverview('24h');
			error = '';
		} catch (e: any) {
			error = e.message ?? 'Could not load traffic';
		}
	}

	$effect(() => pollWhileVisible(() => untrack(load), 60_000));

	let total = $derived(data?.total);
</script>

<section
	aria-labelledby="traffic-heading"
	class="rounded-lg border bg-card p-4"
	data-testid="traffic-tile"
>
	<div class="flex flex-wrap items-baseline justify-between gap-2">
		<h2 id="traffic-heading" class="flex items-center gap-2 text-base font-semibold">
			<GlobeIcon aria-hidden="true" class="size-4 text-muted-foreground" /> Traffic
		</h2>
		<p class="text-sm text-muted-foreground">Last 24 hours</p>
	</div>

	{#if error}
		<p class="mt-2 text-sm text-muted-foreground">
			Traffic is unavailable right now ({error}). It refreshes every minute.
		</p>
	{:else if !data || !total}
		<Skeleton class="mt-3 h-24 w-full" />
	{:else if total.requests === 0}
		<p class="mt-2 text-sm text-muted-foreground">
			No requests yet. Once an app with a domain gets visitors, the busiest apps and any server
			errors show up here.
		</p>
	{:else}
		<dl class="mt-3 grid grid-cols-3 gap-2 text-sm">
			<div>
				<dt class="text-muted-foreground">Requests</dt>
				<dd class="text-lg font-semibold tabular-nums">{formatCount(total.requests)}</dd>
			</div>
			<div>
				<dt class="text-muted-foreground">5xx errors</dt>
				<dd
					class={(total.err_pct ?? 0) >= 1
						? 'text-lg font-semibold text-destructive tabular-nums'
						: 'text-lg font-semibold tabular-nums'}
				>
					{formatPercent(total.err_pct)}
				</dd>
			</div>
			<div>
				<dt class="text-muted-foreground">Latency p95</dt>
				<dd class="text-lg font-semibold tabular-nums">{formatMs(total.p95)}</dd>
			</div>
		</dl>

		<div class="mt-4 grid gap-4 md:grid-cols-2">
			<div class="min-w-0">
				<h3 class="text-sm font-medium">Busiest apps</h3>
				{#if data.busiest.length === 0}
					<p class="mt-1 text-sm text-muted-foreground">
						Only the dashboard and unknown hosts got requests.
					</p>
				{:else}
					<ul class="mt-1 divide-y">
						{#each data.busiest.slice(0, 5) as app (app.id)}
							<li class="flex items-center justify-between gap-3 py-1.5 text-sm">
								<a
									href={appTabHref(app.id, 'metrics')}
									class="min-w-0 truncate font-medium hover:underline">{app.name}</a
								>
								<span class="shrink-0 text-muted-foreground tabular-nums">
									{formatRps(app.rps)} · p95 {formatMs(app.p95)}
								</span>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
			<div class="min-w-0">
				<h3 class="text-sm font-medium">Server errors</h3>
				{#if data.errors.length === 0}
					<p class="mt-1 flex items-center gap-1.5 text-sm text-muted-foreground">
						<CheckIcon aria-hidden="true" class="size-4 text-success" /> No app answered with a 5xx.
					</p>
				{:else}
					<ul class="mt-1 divide-y">
						{#each data.errors as app (app.id)}
							<li class="flex items-center justify-between gap-3 py-1.5 text-sm">
								<a
									href={appTabHref(app.id, 'metrics')}
									class="flex min-w-0 items-center gap-1.5 font-medium hover:underline"
								>
									<AlertIcon aria-hidden="true" class="size-4 shrink-0 text-destructive" />
									<span class="truncate">{app.name}</span>
								</a>
								<span class="shrink-0 text-muted-foreground tabular-nums">
									{formatCount(app.s5xx)} × 5xx · {formatPercent(app.err_pct)}
								</span>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</div>
		{#if data.unknown_requests > 0}
			<p class="mt-3 text-xs text-muted-foreground">
				{formatCount(data.unknown_requests)} request{data.unknown_requests === 1 ? '' : 's'} for hosts
				Tond doesn't serve (scanners, old DNS records).
			</p>
		{/if}
	{/if}
</section>
