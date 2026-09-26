<script lang="ts">
	// Request metrics of an app or preview (tond-proxy traffic): range picker,
	// summary numbers, requests per second stacked by status class, latency
	// percentiles and bandwidth, with deploy / restart / crash markers. Loads
	// and refreshes itself (paused while the browser tab is hidden).
	import { untrack } from 'svelte';
	import {
		getRequestMetrics,
		type RequestMetricsResponse,
		type RequestOwner,
		type RequestRange
	} from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import {
		EVENT_LABELS,
		formatBytes,
		formatCount,
		formatMs,
		formatPercent,
		formatRate,
		formatRps,
		formatTime,
		markerColor,
		maxOf,
		niceBytesMax,
		niceMax,
		refreshInterval,
		stack
	} from '$lib/metrics-format';
	import { cn } from '$lib/utils';
	import * as ButtonGroup from '$lib/components/ui/button-group/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import TimeChart from './time-chart.svelte';
	import ChartLegend from './chart-legend.svelte';
	import GlobeIcon from '~icons/ri/global-line';
	import AlertIcon from '~icons/ri/error-warning-line';

	interface Props {
		owner: RequestOwner;
		id: string;
		/** What is measured, for copy: "app", "preview". */
		noun?: string;
		/** Initial range. */
		initialRange?: RequestRange;
	}

	let { owner, id, noun = 'app', initialRange = '24h' }: Props = $props();

	const RANGES: RequestRange[] = ['1h', '6h', '24h', '7d', '30d', '90d'];
	const RANGE_LABELS: Record<RequestRange, string> = {
		'1h': 'last hour',
		'6h': 'last 6 hours',
		'24h': 'last 24 hours',
		'7d': 'last 7 days',
		'30d': 'last 30 days',
		'90d': 'last 90 days'
	};

	// svelte-ignore state_referenced_locally
	let range = $state<RequestRange>(initialRange);
	let data = $state<RequestMetricsResponse | null>(null);
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state('');

	async function load(r: RequestRange) {
		refreshing = true;
		try {
			const next = await getRequestMetrics(owner, id, r);
			if (r !== range) return;
			data = next;
			error = '';
		} catch (e: any) {
			error = e.message ?? 'Could not load request metrics';
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
	let sum = $derived(data?.summary);
	let markers = $derived(data?.markers ?? []);
	let empty = $derived(!!data && data.summary.requests === 0 && markers.length === 0);

	// 1xx (protocol switches such as WebSocket upgrades) are successes too.
	let ok = $derived(s ? s.rps_2xx.map((v, i) => v + s.rps_1xx[i]) : []);
	let rpsMax = $derived(
		s ? niceMax(Math.max(0.1, ...stack([ok, s.rps_3xx, s.rps_4xx, s.rps_5xx])[3])) : 1
	);
	let latencyMax = $derived(s ? niceMax(Math.max(1, maxOf(s.p99, s.p95, s.p50))) : 1);
	let bandwidthMax = $derived(
		s ? niceBytesMax(Math.max(1024, ...s.bytes_out, ...s.bytes_in)) : 1024
	);
	let errorsHigh = $derived((sum?.err_pct ?? 0) >= 1);
	let markerKinds = $derived([...new Set(markers.map((m) => m.kind))]);
	let bucketLabel = $derived(
		data
			? data.step >= 3600
				? `${data.step / 3600} h buckets`
				: `${data.step / 60} min buckets`
			: ''
	);
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<ButtonGroup.Root aria-label="Request metrics time range">
			{#each RANGES as r (r)}
				<Button
					size="sm"
					variant={range === r ? 'default' : 'outline'}
					aria-pressed={range === r}
					onclick={() => (range = r)}
				>
					{r}
				</Button>
			{/each}
		</ButtonGroup.Root>
		<p class="text-xs text-muted-foreground">
			{#if data}{bucketLabel} · counted by tond-proxy{:else}Counted by tond-proxy{/if}
		</p>
	</div>

	{#if error}
		<InlineError title="Couldn't load request metrics" message={error}>
			{#snippet actions()}
				<Button size="sm" variant="outline" onclick={() => load(range)} disabled={refreshing}>
					{refreshing ? 'Retrying…' : 'Retry'}
				</Button>
			{/snippet}
		</InlineError>
	{/if}

	{#if data && !data.proxy_connected}
		<p class="rounded-md border border-warn/50 bg-warn/5 px-4 py-3 text-sm" role="status">
			tond-proxy isn't answering, so the newest minutes may be missing. Check it with
			<code class="text-xs">systemctl status tond-proxy</code> on the server.
		</p>
	{/if}

	{#if loading}
		<div class="grid grid-cols-2 gap-3 lg:grid-cols-4" aria-busy="true">
			{#each [0, 1, 2, 3] as i (i)}
				<Skeleton class="h-20" />
			{/each}
		</div>
		<Skeleton class="h-56" />
	{:else if empty}
		<Empty.Root class="border border-dashed">
			<Empty.Header>
				<Empty.Media variant="icon"><GlobeIcon /></Empty.Media>
				<Empty.Title>No requests in the {RANGE_LABELS[range]}</Empty.Title>
				<Empty.Description>
					Tond counts every request tond-proxy serves on this {noun}'s domain: status codes, latency
					and bandwidth. {noun === 'app'
						? 'Apps without a domain, such as background workers, have no request metrics.'
						: 'Open the preview URL to see traffic here.'}
				</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else if data && s && sum}
		<div class={cn('space-y-4 transition-opacity', refreshing && 'opacity-70')}>
			<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">Requests · {range}</p>
					<p class="text-xl font-semibold tabular-nums">{formatCount(sum.requests)}</p>
					<p class="text-xs text-muted-foreground">
						{data.current ? `${formatRps(data.current.rps)} now` : `${formatRps(sum.rps)} average`}
					</p>
				</div>
				<div class={cn('rounded-lg border p-3', errorsHigh && 'border-warn/50 bg-warn/5')}>
					<p class="text-xs text-muted-foreground">Server errors (5xx)</p>
					<p class="flex items-center gap-1.5 text-xl font-semibold tabular-nums">
						{#if errorsHigh}<AlertIcon class="size-4 text-warn" aria-hidden="true" />{/if}
						{formatPercent(sum.err_pct)}
					</p>
					<p class="text-xs text-muted-foreground">
						{formatCount(sum.s5xx)} 5xx{sum.s502
							? ` · ${formatCount(sum.s502)} × 502`
							: ''}{sum.s504 ? ` · ${formatCount(sum.s504)} × 504` : ''} · {formatCount(sum.s4xx)} 4xx
					</p>
				</div>
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">Latency p95</p>
					<p class="text-xl font-semibold tabular-nums">{formatMs(sum.p95)}</p>
					<p class="text-xs text-muted-foreground">
						p50 {formatMs(sum.p50)} · p99 {formatMs(sum.p99)}
					</p>
				</div>
				<div class="rounded-lg border p-3">
					<p class="text-xs text-muted-foreground">Bandwidth · {range}</p>
					<p class="text-xl font-semibold tabular-nums">{formatBytes(sum.bytes_out)}</p>
					<p class="text-xs text-muted-foreground">out · {formatBytes(sum.bytes_in)} in</p>
				</div>
			</div>

			{#if markerKinds.length > 0}
				<div class="flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
					{#each markerKinds as kind (kind)}
						<span class="flex items-center gap-1.5">
							<span class="size-2 rounded-full" style:background-color={markerColor(kind)}></span>
							{EVENT_LABELS[kind]}
						</span>
					{/each}
				</div>
			{/if}

			<Card.Root>
				<Card.Header>
					<div class="flex flex-wrap items-start justify-between gap-2">
						<div class="space-y-1.5">
							<Card.Title>Requests</Card.Title>
							<Card.Description>Per second, by response status</Card.Description>
						</div>
						<ChartLegend
							items={[
								{ label: '1xx–2xx', color: 'var(--success)' },
								{ label: '3xx', color: 'var(--info)' },
								{ label: '4xx', color: 'var(--warn)' },
								{ label: '5xx', color: 'var(--destructive)' }
							]}
						/>
					</div>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Requests per second by status"
						times={s.t}
						stacked
						series={[
							{ key: 'ok', label: '1xx–2xx', values: ok, color: 'var(--success)' },
							{ key: '3xx', label: '3xx', values: s.rps_3xx, color: 'var(--info)' },
							{ key: '4xx', label: '4xx', values: s.rps_4xx, color: 'var(--warn)' },
							{ key: '5xx', label: '5xx', values: s.rps_5xx, color: 'var(--destructive)' }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={rpsMax}
						yFormat={formatRps}
						events={markers}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<div class="flex flex-wrap items-start justify-between gap-2">
						<div class="space-y-1.5">
							<Card.Title>Latency</Card.Title>
							<Card.Description>
								Until the last byte, measured at the proxy · your {noun}'s own share (p95): {formatMs(
									sum.upstream_p95
								)}
							</Card.Description>
						</div>
						<ChartLegend
							items={[
								{ label: 'p50', color: 'var(--chart-2)' },
								{ label: 'p95', color: 'var(--chart-1)' },
								{ label: 'p99', color: 'var(--chart-3)', dashed: true }
							]}
						/>
					</div>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Response time percentiles"
						times={s.t}
						series={[
							{ key: 'p50', label: 'p50', values: s.p50, color: 'var(--chart-2)' },
							{ key: 'p95', label: 'p95', values: s.p95, color: 'var(--chart-1)' },
							{ key: 'p99', label: 'p99', values: s.p99, color: 'var(--chart-3)', dashed: true }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={latencyMax}
						yFormat={formatMs}
						events={markers}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<div class="flex flex-wrap items-start justify-between gap-2">
						<div class="space-y-1.5">
							<Card.Title>Bandwidth</Card.Title>
							<Card.Description>Request and response bodies, bytes per second</Card.Description>
						</div>
						<ChartLegend
							items={[
								{ label: 'Out', color: 'var(--chart-1)' },
								{ label: 'In', color: 'var(--chart-2)', dashed: true }
							]}
						/>
					</div>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Bandwidth"
						times={s.t}
						series={[
							{
								key: 'out',
								label: 'out',
								values: s.bytes_out,
								color: 'var(--chart-1)',
								area: true
							},
							{ key: 'in', label: 'in', values: s.bytes_in, color: 'var(--chart-2)', dashed: true }
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={bandwidthMax}
						yFormat={(v) => formatRate(v)}
						events={markers}
					/>
				</Card.Content>
			</Card.Root>

			{#if markers.length > 0}
				<Card.Root>
					<Card.Header>
						<Card.Title>Deploys and restarts</Card.Title>
						<Card.Description>The {RANGE_LABELS[range]}, newest first</Card.Description>
					</Card.Header>
					<Card.Content>
						<ul class="divide-y text-sm">
							{#each [...markers].reverse() as m (`${m.t}-${m.kind}-${m.deployment_id ?? ''}`)}
								<li class="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
									<span class="inline-flex items-center gap-1.5 font-medium">
										<span class="size-2 rounded-full" style:background-color={markerColor(m.kind)}
										></span>
										{EVENT_LABELS[m.kind]}
									</span>
									<span class="text-muted-foreground tabular-nums"
										>{formatTime(m.t, range, true)}</span
									>
									{#if m.detail}
										<span class="text-muted-foreground">{m.detail}</span>
									{/if}
									{#if m.deployment_id && owner === 'apps'}
										<a
											href="/apps/{id}/deployments/{m.deployment_id}"
											class="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
											>View deploy</a
										>
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
