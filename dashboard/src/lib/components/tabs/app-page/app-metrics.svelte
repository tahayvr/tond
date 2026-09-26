<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import * as Chart from '$lib/components/ui/chart/index.js';
	import { AreaChart, Area, LinearGradient } from 'layerchart';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import type { MetricPoint } from '$lib/api';

	interface Props {
		metrics: MetricPoint[];
		loading: boolean;
		error: string;
		period: '1h' | '24h' | '7d';
		onPeriodChange: (period: '1h' | '24h' | '7d') => void;
	}

	let { metrics, loading, error, period, onPeriodChange }: Props = $props();

	const metricsChartConfig = {
		cpu_percent: { label: 'CPU %', color: 'var(--chart-1)' },
		mem_percent: { label: 'Memory %', color: 'var(--chart-2)' }
	} satisfies Chart.ChartConfig;

	const netChartConfig = {
		net_rx: { label: 'RX (bytes)', color: 'var(--chart-3)' },
		net_tx: { label: 'TX (bytes)', color: 'var(--chart-4)' }
	} satisfies Chart.ChartConfig;

	// Convert collected_at strings to Date objects so LayerChart uses scaleTime
	let metricsChartData = $derived(
		metrics.map((m) => ({ ...m, collected_at: new Date(m.collected_at) }))
	);

	function xAxisFormat(d: Date): string {
		if (period === '7d') {
			return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
		}
		return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}
</script>

{#snippet gradientAreas(series: { key: string; color?: string }[])}
	{#each series as s (s.key)}
		<LinearGradient
			stops={[s.color ?? '', 'color-mix(in lch, ' + s.color + ' 10%, transparent)']}
			vertical
		>
			{#snippet children({ gradient })}
				<Area seriesKey={s.key} fill={gradient} fillOpacity={0.4} line={{ class: 'stroke-1' }} />
			{/snippet}
		</LinearGradient>
	{/each}
{/snippet}

<div class="space-y-6 pt-4">
	{#if error}
		<ErrorAlert title="Error" description={error} />
	{/if}

	<!-- Period selector -->
	<div class="flex items-center gap-2">
		<span class="text-sm text-muted-foreground">Period:</span>
		{#each ['1h', '24h', '7d'] as const as p (p)}
			<Button
				size="sm"
				variant={period === p ? 'default' : 'outline'}
				onclick={() => onPeriodChange(p)}
			>
				{p}
			</Button>
		{/each}
	</div>

	{#if loading}
		<div class="py-8 text-center text-muted-foreground">Loading metrics...</div>
	{:else if metrics.length === 0}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No metrics yet</p>
			<p class="mt-1 text-sm text-muted-foreground">
				Data is collected every 30 seconds while the app is running
			</p>
		</div>
	{:else}
		<!-- CPU & Memory chart -->
		<Card.Root>
			<Card.Header>
				<Card.Title>CPU & Memory Usage</Card.Title>
				<Card.Description>Percentage over time</Card.Description>
			</Card.Header>
			<Card.Content>
				<Chart.Container config={metricsChartConfig} class="h-55 w-full">
					<AreaChart
						data={metricsChartData}
						x="collected_at"
						yDomain={[0, 100]}
						series={[
							{
								key: 'cpu_percent',
								label: metricsChartConfig.cpu_percent.label,
								color: metricsChartConfig.cpu_percent.color
							},
							{
								key: 'mem_percent',
								label: metricsChartConfig.mem_percent.label,
								color: metricsChartConfig.mem_percent.color
							}
						]}
						seriesLayout="stack"
						props={{
							xAxis: {
								format: xAxisFormat
							},
							yAxis: {
								format: (v: number) => `${v}%`
							}
						}}
					>
						{#snippet tooltip()}
							<Chart.Tooltip labelFormatter={(v) => xAxisFormat(v as Date)} />
						{/snippet}
						{#snippet marks({ context })}
							{@render gradientAreas(context.series.visibleSeries)}
						{/snippet}
					</AreaChart>
				</Chart.Container>
			</Card.Content>
		</Card.Root>

		<!-- Network I/O chart -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Network I/O</Card.Title>
				<Card.Description>Cumulative bytes received / transmitted</Card.Description>
			</Card.Header>
			<Card.Content>
				<Chart.Container config={netChartConfig} class="h-55 w-full">
					<AreaChart
						data={metricsChartData}
						x="collected_at"
						series={[
							{
								key: 'net_rx',
								label: netChartConfig.net_rx.label,
								color: netChartConfig.net_rx.color
							},
							{
								key: 'net_tx',
								label: netChartConfig.net_tx.label,
								color: netChartConfig.net_tx.color
							}
						]}
						seriesLayout="stack"
						props={{
							xAxis: {
								format: xAxisFormat
							}
						}}
					>
						{#snippet tooltip()}
							<Chart.Tooltip labelFormatter={(v) => xAxisFormat(v as Date)} />
						{/snippet}
						{#snippet marks({ context })}
							{@render gradientAreas(context.series.visibleSeries)}
						{/snippet}
					</AreaChart>
				</Chart.Container>
			</Card.Content>
		</Card.Root>

		<!-- Restart count summary -->
		{@const latestRestarts = metrics.at(-1)?.restarts ?? 0}
		<Card.Root>
			<Card.Header>
				<Card.Title>Container Restarts</Card.Title>
			</Card.Header>
			<Card.Content>
				<p class="text-3xl font-bold tabular-nums">{latestRestarts}</p>
				<p class="text-sm text-muted-foreground">total restarts recorded</p>
			</Card.Content>
		</Card.Root>
	{/if}
</div>
