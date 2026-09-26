<!--
  HostHistoryPanel — the host over time: CPU, load, memory and swap, disk and
  inodes of the data directory's filesystem (and `/` when it is separate).
  One row a minute is recorded from the metrics collector's existing tick;
  30-day ranges come from hourly rollups.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { getHostHistory, type HostHistory, type MetricEvent, type MetricsRange } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { formatBytes, niceBytesMax, niceMax, refreshInterval } from '$lib/metrics-format';
	import { cn } from '$lib/utils';
	import TimeChart from '$lib/components/metrics/time-chart.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import * as ButtonGroup from '$lib/components/ui/button-group/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import HistoryIcon from '~icons/ri/history-line';

	const RANGES: MetricsRange[] = ['1h', '24h', '7d', '30d'];

	let range = $state<MetricsRange>('24h');
	let data = $state<HostHistory | null>(null);
	let loading = $state(true);
	let refreshing = $state(false);
	let error = $state('');

	async function load(r: MetricsRange) {
		refreshing = true;
		try {
			const next = await getHostHistory(r);
			if (r !== range) return;
			data = next;
			error = '';
		} catch (e: any) {
			error = e.message ?? 'request failed';
		} finally {
			refreshing = false;
			loading = false;
		}
	}

	$effect(() => {
		const r = range;
		return pollWhileVisible(() => untrack(() => load(r)), refreshInterval(r));
	});

	let s = $derived(data?.series);
	let hasData = $derived((s?.t.length ?? 0) > 0);
	let noEvents: MetricEvent[] = [];

	let cpuMax = $derived(Math.max(100, niceMax(Math.max(0, ...(s?.cpu_max ?? [0])))));
	let loadMax = $derived(niceMax(Math.max(1, ...(s?.load1 ?? [0]), ...(s?.load5 ?? [0])) * 1.1));
	let memTotal = $derived(s && s.mem_total.length ? s.mem_total[s.mem_total.length - 1] : 0);
	let memMax = $derived(niceBytesMax(Math.max(memTotal, ...(s?.mem_used ?? [0]), 1)));
	let swapTotal = $derived(s && s.swap_total.length ? s.swap_total[s.swap_total.length - 1] : 0);
	let diskTotal = $derived(s && s.disk_total.length ? s.disk_total[s.disk_total.length - 1] : 0);
	let rootTotal = $derived(s && s.root_total.length ? s.root_total[s.root_total.length - 1] : 0);
	let inodePercent = $derived(
		s ? s.inodes_used.map((u, i) => (s.inodes_total[i] ? (u / s.inodes_total[i]) * 100 : 0)) : []
	);
	let rootInodePercent = $derived(
		s
			? s.root_inodes_used.map((u, i) =>
					s.root_inodes_total[i] ? (u / s.root_inodes_total[i]) * 100 : 0
				)
			: []
	);

	let hasInodes = $derived((s?.inodes_total ?? []).some((v) => v > 0));

	let stepLabel = $derived(
		!data
			? ''
			: data.step >= 3600
				? `${data.step / 3600} h averages`
				: `${data.step / 60} min averages`
	);
</script>

<div class="space-y-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<ButtonGroup.Root aria-label="Time range">
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
			{hasData ? `${stepLabel} · ` : ''}recorded every minute, kept 48 h; hourly for a year
		</p>
	</div>

	<InlineError message={error ? `Couldn't load host history: ${error}` : ''} />

	{#if loading}
		<Skeleton class="h-56" />
		<Skeleton class="h-56" />
	{:else if data && s && !hasData}
		<Empty.Root class="border border-dashed">
			<Empty.Header>
				<Empty.Media variant="icon"><HistoryIcon /></Empty.Media>
				<Empty.Title>No history for this range yet</Empty.Title>
				<Empty.Description>
					Tond records the host every minute. The first points appear a minute or two after
					tond-server starts.
				</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else if data && s}
		<div class={cn('grid gap-4 xl:grid-cols-2', refreshing && 'opacity-70 transition-opacity')}>
			<Card.Root>
				<Card.Header>
					<Card.Title>CPU</Card.Title>
					<Card.Description>Average of all cores; dashed: busiest sample</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Host CPU"
						times={s.t}
						series={[
							{ key: 'cpu', label: 'Average', values: s.cpu, color: 'var(--chart-1)', area: true },
							{
								key: 'max',
								label: 'Peak',
								values: s.cpu_max,
								color: 'var(--chart-3)',
								dashed: true
							}
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={cpuMax}
						yFormat={(v) => `${Math.round(v)}%`}
						events={noEvents}
						height={170}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Load average</Card.Title>
					<Card.Description>Runnable tasks over 1, 5 and 15 minutes</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Load average"
						times={s.t}
						series={[
							{ key: 'l1', label: '1 min', values: s.load1, color: 'var(--chart-1)' },
							{ key: 'l5', label: '5 min', values: s.load5, color: 'var(--chart-2)' },
							{
								key: 'l15',
								label: '15 min',
								values: s.load15,
								color: 'var(--chart-4)',
								dashed: true
							}
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={loadMax}
						yFormat={(v) => v.toFixed(v >= 10 ? 0 : 1)}
						events={noEvents}
						height={170}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Memory</Card.Title>
					<Card.Description>
						Used of {formatBytes(memTotal)}{swapTotal > 0
							? `; swap of ${formatBytes(swapTotal)}`
							: '; no swap'}
					</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Host memory"
						times={s.t}
						series={[
							{
								key: 'mem',
								label: 'Memory',
								values: s.mem_used,
								color: 'var(--chart-2)',
								area: true
							},
							...(swapTotal > 0
								? [
										{
											key: 'swap',
											label: 'Swap',
											values: s.swap_used,
											color: 'var(--chart-5)',
											dashed: true
										}
									]
								: [])
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={memMax}
						yFormat={(v) => formatBytes(v, 0)}
						limit={memTotal ? { value: memTotal, label: 'total' } : null}
						events={noEvents}
						height={170}
					/>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Disk</Card.Title>
					<Card.Description>
						Used on {data.data_mount} (Tond's data){data.separate_root ? ' and on /' : ''}
					</Card.Description>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Disk used"
						times={s.t}
						series={[
							{
								key: 'disk',
								label: data.data_mount,
								values: s.disk_used,
								color: 'var(--chart-3)',
								area: true
							},
							...(data.separate_root
								? [
										{
											key: 'root',
											label: '/',
											values: s.root_used,
											color: 'var(--chart-4)',
											dashed: true
										}
									]
								: [])
						]}
						from={data.from}
						to={data.to}
						step={data.step}
						{range}
						yMax={niceBytesMax(Math.max(diskTotal, rootTotal, 1))}
						yFormat={(v) => formatBytes(v, 0)}
						limit={diskTotal ? { value: diskTotal, label: 'size' } : null}
						events={noEvents}
						height={170}
					/>
				</Card.Content>
			</Card.Root>

			{#if hasInodes}
				<Card.Root class="xl:col-span-2">
					<Card.Header>
						<Card.Title>Inodes</Card.Title>
						<Card.Description>
							Files and folders use one each; a disk can run out of inodes before it runs out of
							space (many small files: caches, mail, node_modules).
						</Card.Description>
					</Card.Header>
					<Card.Content>
						<TimeChart
							label="Inodes used"
							times={s.t}
							series={[
								{
									key: 'inodes',
									label: data.data_mount,
									values: inodePercent,
									color: 'var(--chart-5)',
									area: true
								},
								...(data.separate_root
									? [
											{
												key: 'root_inodes',
												label: '/',
												values: rootInodePercent,
												color: 'var(--chart-4)',
												dashed: true
											}
										]
									: [])
							]}
							from={data.from}
							to={data.to}
							step={data.step}
							{range}
							yMax={Math.max(10, niceMax(Math.max(...inodePercent, ...rootInodePercent, 1) * 1.2))}
							yFormat={(v) => `${v.toFixed(v < 10 ? 1 : 0)}%`}
							events={noEvents}
							height={140}
						/>
					</Card.Content>
				</Card.Root>
			{:else}
				<p class="text-xs text-muted-foreground xl:col-span-2">
					This filesystem doesn't have a fixed number of inodes (btrfs, ZFS and similar grow them as
					needed), so there is nothing to run out of.
				</p>
			{/if}
		</div>
	{/if}
</div>
