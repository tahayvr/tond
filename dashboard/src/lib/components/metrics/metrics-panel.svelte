<script lang="ts">
	// Resource usage of one app / database / cron job: range picker, current
	// values, separate CPU / memory / network charts (network as a rate), and
	// restart / crash / OOM markers. Loads and refreshes itself (paused while
	// the tab is hidden).
	import { untrack } from 'svelte';
	import { getMetrics, type MetricsOwner, type MetricsRange, type MetricsResponse } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import {
		EVENT_LABELS,
		countEvents,
		formatBytes,
		formatCpu,
		formatRate,
		formatTime,
		niceBytesMax,
		niceMax,
		refreshInterval
	} from '$lib/metrics-format';
	import { cn } from '$lib/utils';
	import * as ButtonGroup from '$lib/components/ui/button-group/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import TimeChart from './time-chart.svelte';
	import PulseIcon from '~icons/ri/pulse-line';
	import RestartIcon from '~icons/ri/restart-line';
	import AlertIcon from '~icons/ri/error-warning-line';

	interface Props {
		owner: MetricsOwner;
		id: string;
		/** What is measured, for copy: "app", "database", "scheduled task". */
		noun?: string;
	}

	let { owner, id, noun = 'app' }: Props = $props();

	const RANGES: { value: MetricsRange; label: string }[] = [
		{ value: '1h', label: '1h' },
		{ value: '24h', label: '24h' },
		{ value: '7d', label: '7d' },
		{ value: '30d', label: '30d' }
	];

	let range = $state<MetricsRange>('1h');
	let data = $state<MetricsResponse | null>(null);
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state('');

	async function load(r: MetricsRange) {
		refreshing = true;
		try {
			const next = await getMetrics(owner, id, r);
			// Ignore a response for a range the user already switched away from.
			if (r !== range) return;
			data = next;
			error = '';
		} catch (e: any) {
			error = e.message ?? 'Could not load metrics';
		} finally {
			refreshing = false;
			loading = false;
		}
	}

	$effect(() => {
		const r = range;
		void owner;
		void id;
		return pollWhileVisible(() => untrack(() => load(r)), refreshInterval(r));
	});

	let s = $derived(data?.series);
	let hasData = $derived((s?.t.length ?? 0) > 0);
	let counts = $derived(countEvents(data?.events ?? []));

	let cpuMax = $derived(Math.max(100, niceMax(Math.max(0, ...(s?.cpu_max ?? [0])))));
	let memMax = $derived(
		niceBytesMax(Math.max(data?.mem_limit ?? 0, ...(s?.mem_max ?? [0]), 1) * 1.05)
	);
	let netMax = $derived(niceBytesMax(Math.max(1024, ...(s?.rx ?? [0]), ...(s?.tx ?? [0]))));

	let rangeLabel = $derived(
		{ '1h': 'last hour', '24h': 'last 24 hours', '7d': 'last 7 days', '30d': 'last 30 days' }[range]
	);
	let restartsTotal = $derived(counts.restarts + counts.crashes);
</script>

<div class="space-y-4 pt-4">
	<!-- Range picker scopes everything below -->
	<div class="flex flex-wrap items-center justify-between gap-3">
		<ButtonGroup.Root aria-label="Time range">
			{#each RANGES as r (r.value)}
				<Button
					size="sm"
					variant={range === r.value ? 'default' : 'outline'}
					aria-pressed={range === r.value}
					onclick={() => (range = r.value)}
				>
					{r.label}
				</Button>
			{/each}
		</ButtonGroup.Root>
		<p class="text-xs text-muted-foreground">
			{#if data && hasData}
				{data.step >= 3600
					? `${data.step / 3600} h`
					: data.step >= 60
						? `${data.step / 60} min`
						: `${data.step} s`} averages · sampled every 30 s
			{:else}
				Sampled every 30 seconds
			{/if}
		</p>
	</div>

	{#if error}
		<div
			class="flex items-center justify-between gap-3 rounded-md bg-destructive/10 px-4 py-3 text-sm text-destructive"
			role="alert"
		>
			<span>Couldn't load metrics: {error}</span>
			<Button size="sm" variant="outline" onclick={() => load(range)} disabled={refreshing}>
				{refreshing ? 'Retrying…' : 'Retry'}
			</Button>
		</div>
	{/if}

	{#if loading}
		<div class="grid grid-cols-2 gap-3 lg:grid-cols-4" aria-busy="true">
			{#each [0, 1, 2, 3] as i (i)}
				<Skeleton class="h-20" />
			{/each}
		</div>
		<Skeleton class="h-56" />
		<Skeleton class="h-56" />
	{:else if data && !hasData && data.events.length === 0}
		<Empty.Root class="border border-dashed">
			<Empty.Header>
				<Empty.Media variant="icon"><PulseIcon /></Empty.Media>
				<Empty.Title>No data for the {rangeLabel}</Empty.Title>
				<Empty.Description>
					Tond records CPU, memory and network every 30 seconds while this {noun} is running. Charts appear
					after the first minute.
				</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else if data && s}
		<div class={cn('space-y-4 transition-opacity', refreshing && 'opacity-70')}>
			<!-- Current values -->
			<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">CPU now</p>
					<p class="text-xl font-semibold tabular-nums">
						{data.current ? formatCpu(data.current.cpu) : '—'}
					</p>
					<p class="text-xs text-muted-foreground">100% = one core</p>
				</div>
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">Memory now</p>
					<p class="text-xl font-semibold tabular-nums">
						{data.current ? formatBytes(data.current.mem) : '—'}
					</p>
					<p class="text-xs text-muted-foreground">
						{data.mem_limit ? `of ${formatBytes(data.mem_limit)} limit` : 'no limit'}{data.current
							?.mem_peak
							? ` · peak ${formatBytes(data.current.mem_peak)}`
							: ''}
					</p>
				</div>
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">Network now</p>
					<p class="text-xl font-semibold tabular-nums">
						{data.current ? formatRate(data.current.rx) : '—'}
					</p>
					<p class="text-xs text-muted-foreground">
						in{data.current ? ` · ${formatRate(data.current.tx)} out` : ''}
					</p>
				</div>
				<div
					class={cn(
						'rounded-lg border p-3',
						(restartsTotal > 0 || counts.ooms > 0) && 'border-warn/50 bg-warn/5'
					)}
				>
					<p class="text-xs text-muted-foreground">Restarts · {range}</p>
					<p class="flex items-center gap-1.5 text-xl font-semibold tabular-nums">
						{#if restartsTotal > 0 || counts.ooms > 0}
							<AlertIcon class="size-4 text-warn" aria-hidden="true" />
						{/if}
						{restartsTotal}
					</p>
					<p class="text-xs text-muted-foreground">
						{counts.ooms} out-of-memory kill{counts.ooms === 1 ? '' : 's'}
					</p>
				</div>
			</div>

			{#if data.events.length > 0}
				<div class="flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
					<span class="flex items-center gap-1.5">
						<span class="size-2 rounded-full bg-warn"></span>Restart
					</span>
					<span class="flex items-center gap-1.5">
						<span class="size-2 rounded-full bg-destructive"></span>Crash or out of memory
					</span>
				</div>
			{/if}

			<Card.Root>
				<Card.Header>
					<Card.Title>CPU</Card.Title>
					<Card.Description>Percent of one core, averaged per bucket</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="CPU usage"
						times={s.t}
						series={[
							{ key: 'cpu', label: 'CPU', values: s.cpu, color: 'var(--chart-1)', area: true }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={cpuMax}
						yFormat={(v) => `${Math.round(v)}%`}
						events={data.events}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Memory</Card.Title>
					<Card.Description>
						Working set{data.mem_limit ? `, limit ${formatBytes(data.mem_limit)}` : ''}
					</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Memory usage"
						times={s.t}
						series={[
							{ key: 'mem', label: 'Memory', values: s.mem, color: 'var(--chart-2)', area: true }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={memMax}
						yFormat={(v) => formatBytes(v, 0)}
						limit={data.mem_limit ? { value: data.mem_limit, label: 'limit' } : null}
						events={data.events}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<div class="flex flex-wrap items-start justify-between gap-2">
						<div class="space-y-1.5">
							<Card.Title>Network</Card.Title>
							<Card.Description>Bytes per second</Card.Description>
						</div>
						<div class="flex items-center gap-4 text-xs text-muted-foreground">
							<span class="flex items-center gap-1.5">
								<svg width="16" height="4" aria-hidden="true"
									><line
										x1="0"
										x2="16"
										y1="2"
										y2="2"
										stroke="var(--chart-2)"
										stroke-width="2"
									/></svg
								>In
							</span>
							<span class="flex items-center gap-1.5">
								<svg width="16" height="4" aria-hidden="true"
									><line
										x1="0"
										x2="16"
										y1="2"
										y2="2"
										stroke="var(--chart-1)"
										stroke-width="2"
										stroke-dasharray="4 2"
									/></svg
								>Out
							</span>
						</div>
					</div>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Network traffic"
						times={s.t}
						series={[
							{ key: 'rx', label: 'in', values: s.rx, color: 'var(--chart-2)' },
							{ key: 'tx', label: 'out', values: s.tx, color: 'var(--chart-1)', dashed: true }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={netMax}
						yFormat={(v) => formatRate(v)}
						events={data.events}
					/>
				</Card.Content>
			</Card.Root>

			{#if data.events.length > 0}
				<Card.Root>
					<Card.Header>
						<Card.Title>Restarts and crashes</Card.Title>
						<Card.Description>The {rangeLabel}, newest first</Card.Description>
					</Card.Header>
					<Card.Content>
						<ul class="divide-y text-sm">
							{#each [...data.events].reverse() as ev (`${ev.t}-${ev.kind}`)}
								<li class="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
									<span
										class={cn(
											'inline-flex items-center gap-1.5 font-medium',
											ev.kind === 'restart' ? 'text-foreground' : 'text-destructive'
										)}
									>
										<RestartIcon
											class={cn('size-4', ev.kind === 'restart' ? 'text-warn' : 'text-destructive')}
											aria-hidden="true"
										/>
										{EVENT_LABELS[ev.kind]}
									</span>
									<span class="text-muted-foreground tabular-nums"
										>{formatTime(ev.t, range, true)}</span
									>
									{#if ev.detail}
										<span class="text-muted-foreground">{ev.detail}</span>
									{/if}
								</li>
							{/each}
						</ul>
					</Card.Content>
				</Card.Root>
			{/if}
		</div>
	{/if}
</div>
