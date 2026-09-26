<script lang="ts">
	// Last 24 hours at a glance on the app's Overview: requests per second,
	// 5xx rate, p95 latency, CPU and memory as sparklines (uptime has its own
	// summary above, UptimeBadge).
	import { untrack } from 'svelte';
	import {
		getMetrics,
		getRequestMetrics,
		type MetricsResponse,
		type RequestMetricsResponse
	} from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import {
		formatBytes,
		formatCount,
		formatCpu,
		formatMs,
		formatPercent,
		formatRps
	} from '$lib/metrics-format';
	import { appTabHref } from '$lib/tabs';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import Sparkline from '$lib/components/metrics/sparkline.svelte';

	interface Props {
		appId: string;
	}

	let { appId }: Props = $props();

	let requests = $state<RequestMetricsResponse | null>(null);
	let resources = $state<MetricsResponse | null>(null);
	let loaded = $state(false);
	let failed = $state(false);

	async function load() {
		const [r, m] = await Promise.allSettled([
			getRequestMetrics('apps', appId, '24h'),
			getMetrics('apps', appId, '24h')
		]);
		if (r.status === 'fulfilled') requests = r.value;
		if (m.status === 'fulfilled') resources = m.value;
		failed = r.status === 'rejected' && m.status === 'rejected';
		loaded = true;
	}

	$effect(() => {
		void appId;
		return pollWhileVisible(() => untrack(load), 60_000);
	});

	let rq = $derived(requests?.series);
	let rs = $derived(resources?.series);
	let errPct = $derived(requests?.summary.err_pct ?? null);
</script>

<section aria-labelledby="overview-metrics-heading" class="space-y-2">
	<div class="flex items-baseline justify-between gap-2">
		<h2
			id="overview-metrics-heading"
			class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
		>
			Last 24 hours
		</h2>
		<a
			href={appTabHref(appId, 'metrics')}
			class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
			>All metrics →</a
		>
	</div>
	{#if !loaded}
		<div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5" aria-busy="true">
			{#each [0, 1, 2, 3, 4] as i (i)}
				<Skeleton class="h-24" />
			{/each}
		</div>
	{:else if failed}
		<p class="rounded-md border border-dashed px-3 py-2 text-sm text-muted-foreground">
			Metrics are unavailable right now. They refresh every minute.
		</p>
	{:else}
		<div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
			<Sparkline
				label="Requests"
				value={requests?.current
					? formatRps(requests.current.rps)
					: formatRps(requests?.summary.rps ?? 0)}
				hint={requests?.current ? 'now' : '24 h average'}
				values={rq?.rps ?? []}
				color="var(--chart-2)"
			/>
			<Sparkline
				label="5xx errors"
				value={formatPercent(errPct)}
				hint={requests ? `${formatCount(requests.summary.s5xx)} in 24 h` : undefined}
				values={rq?.err_pct ?? []}
				color="var(--destructive)"
				max={Math.max(5, ...(rq?.err_pct ?? []).map((v) => v ?? 0))}
				warn={(errPct ?? 0) >= 1}
			/>
			<Sparkline
				label="Latency p95"
				value={formatMs(requests?.summary.p95)}
				hint="24 h"
				values={rq?.p95 ?? []}
				color="var(--chart-1)"
			/>
			<Sparkline
				label="CPU"
				value={resources?.current ? formatCpu(resources.current.cpu) : '—'}
				hint="now · 100% = one core"
				values={rs?.cpu ?? []}
				color="var(--chart-3)"
			/>
			<Sparkline
				label="Memory"
				value={resources?.current ? formatBytes(resources.current.mem) : '—'}
				hint={resources?.mem_limit ? `of ${formatBytes(resources.mem_limit)}` : 'now'}
				values={rs?.mem ?? []}
				color="var(--chart-4)"
			/>
		</div>
	{/if}
</section>
