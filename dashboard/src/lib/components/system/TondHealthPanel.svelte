<script lang="ts">
	import { untrack } from 'svelte';
	import { getSystemSelf, type LoopStats, type SystemSelf } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { cn } from '$lib/utils';
	import {
		formatBytes,
		formatMs,
		formatPercent,
		formatUptime,
		loopHealth,
		loopHealthText,
		loopLabel,
		relativeTime
	} from '$lib/tondHealth';
	import MetricCard from '$lib/components/system/metric-card.svelte';
	import HeartPulse from '~icons/ri/heart-pulse-line';
	import Cpu from '~icons/ri/cpu-line';
	import Route from '~icons/ri/route-line';
	import Database from '~icons/ri/database-2-line';

	interface Props {
		/** Refresh interval while the tab is visible */
		intervalMs?: number;
	}

	let { intervalMs = 10_000 }: Props = $props();

	let data = $state<SystemSelf | null>(null);
	let error = $state('');
	let loading = $state(true);
	let now = $state(Date.now());

	async function load() {
		try {
			data = await getSystemSelf();
			error = '';
		} catch (e: any) {
			error = `Couldn't load Tond's own health: ${e.message ?? 'request failed'}. It retries every ${Math.round(intervalMs / 1000)} s.`;
		} finally {
			loading = false;
			now = Date.now();
		}
	}

	$effect(() => {
		void intervalMs;
		return pollWhileVisible(() => untrack(load), intervalMs);
	});

	interface LoopRow extends LoopStats {
		process: 'server' | 'proxy';
	}

	let loops = $derived.by<LoopRow[]>(() => {
		if (!data) return [];
		return [
			...data.loops.map((l) => ({ ...l, process: 'server' as const })),
			...(data.proxy.stats?.loops ?? []).map((l) => ({ ...l, process: 'proxy' as const }))
		];
	});
	let failing = $derived(loops.filter((l) => loopHealth(l) === 'failing').length);
	let proxy = $derived(data?.proxy.stats ?? null);

	const dot: Record<string, string> = {
		ok: 'bg-success',
		slow: 'bg-warn',
		failing: 'bg-destructive'
	};
</script>

<section class="space-y-4" aria-labelledby="tond-health-title">
	<div class="flex flex-col gap-1 sm:flex-row sm:items-end sm:justify-between sm:gap-4">
		<div>
			<h3 id="tond-health-title" class="flex items-center gap-2 text-lg font-bold">
				<HeartPulse class="size-5 text-muted-foreground" aria-hidden="true" />
				Tond health
			</h3>
			<p class="text-sm text-muted-foreground">
				How Tond itself is doing: its memory, CPU and background work.
			</p>
		</div>
		{#if data}
			<p class="font-mono text-[10px] text-muted-foreground/60">
				v{data.version} · up {formatUptime(data.server.uptime_secs)}
			</p>
		{/if}
	</div>

	{#if error}
		<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
			{error}
		</div>
	{/if}

	{#if loading && !data}
		<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4" aria-busy="true">
			{#each [0, 1, 2, 3] as i (i)}
				<div class="h-28 animate-pulse rounded-lg border border-border bg-muted/40"></div>
			{/each}
		</div>
	{:else if data}
		<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
			<MetricCard
				value={formatBytes(data.server.rss_bytes)}
				label="SERVER_MEMORY"
				icon={HeartPulse}
				detail="{data.server.threads ?? '–'} threads · {data.server.open_fds ?? '–'} open files"
			/>
			<MetricCard
				value={formatPercent(data.server.cpu_percent)}
				label="SERVER_CPU"
				icon={Cpu}
				detail="{data.server.tokio_alive_tasks} tasks · builds {data.build_queue
					.active} running, {data.build_queue.queued} queued"
			/>
			<MetricCard
				value={proxy ? formatBytes(proxy.rss_bytes) : '–'}
				label="PROXY"
				icon={Route}
				detail={proxy
					? `${formatPercent(data.proxy.cpu_percent)} CPU · ${proxy.routes} routes`
					: 'Not reporting'}
				healthy={data.proxy.connected}
				healthyDetail={data.proxy.connected
					? 'Connected: route changes apply instantly'
					: 'Not connected: routes update within a minute'}
			/>
			<MetricCard
				value={formatBytes(data.db.size_bytes)}
				label="DATABASE"
				icon={Database}
				detail="WAL {formatBytes(data.db.wal_bytes)} · {data.db.slow_total} slow {data.db
					.slow_total === 1
					? 'query'
					: 'queries'}"
			/>
		</div>

		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm">
				<caption class="sr-only">Background loops</caption>
				<thead>
					<tr class="border-b bg-muted/50 text-left">
						<th class="px-3 py-2 sm:px-4 font-medium text-muted-foreground">
							Background work
							{#if failing > 0}
								<span class="ml-1 text-destructive">· {failing} failing</span>
							{/if}
						</th>
						<th class="px-3 py-2 sm:px-4 font-medium text-muted-foreground">Last run</th>
						<th class="hidden px-3 py-2 sm:px-4 font-medium text-muted-foreground sm:table-cell">Took</th>
						<th class="hidden px-3 py-2 sm:px-4 font-medium text-muted-foreground md:table-cell">
							Next check
						</th>
						<th class="px-3 py-2 sm:px-4 text-right font-medium text-muted-foreground">Errors</th>
					</tr>
				</thead>
				<tbody>
					{#each loops as loop (`${loop.process}:${loop.name}`)}
						{@const health = loopHealth(loop)}
						<tr class="border-b last:border-0" data-loop={loop.name}>
							<td class="px-3 py-2 sm:px-4">
								<div class="flex items-center gap-2">
									<span
										class={cn('inline-block size-2 shrink-0 rounded-full', dot[health])}
										title={loopHealthText(health)}
										aria-label={loopHealthText(health)}
									></span>
									<span>{loopLabel(loop.name)}</span>
									{#if loop.process === 'proxy'}
										<span class="font-mono text-[10px] text-muted-foreground/70 uppercase"
											>proxy</span
										>
									{/if}
								</div>
								{#if loop.last_error}
									<p
										class="mt-1 max-w-md truncate text-xs text-destructive"
										title={loop.last_error}
									>
										{loop.last_error}
									</p>
								{/if}
							</td>
							<td class="px-3 py-2 sm:px-4 whitespace-nowrap text-muted-foreground">
								{relativeTime(loop.last_run_at, now)}
							</td>
							<td class="hidden px-3 py-2 sm:px-4 text-muted-foreground tabular-nums sm:table-cell">
								{formatMs(loop.last_duration_ms)}
							</td>
							<td class="hidden px-3 py-2 sm:px-4 whitespace-nowrap text-muted-foreground md:table-cell">
								{loop.next_wake_at ? relativeTime(loop.next_wake_at, now) : 'on change'}
							</td>
							<td
								class={cn(
									'px-3 py-2 sm:px-4 text-right tabular-nums',
									loop.errors > 0 ? 'text-destructive' : 'text-muted-foreground'
								)}
							>
								{loop.errors}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<p class="text-xs text-muted-foreground/80">
			Loops sleep until they have work, so “next check” can be hours away on a quiet server.
			Prometheus metrics: <code class="font-mono"
				>curl --unix-socket /opt/tond/data/tond.sock http://tond/metrics</code
			> on the server.
		</p>
	{/if}
</section>
