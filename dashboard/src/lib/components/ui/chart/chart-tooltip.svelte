<script lang="ts">
	import { cn, type WithElementRef, type WithoutChildren } from '$lib/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';
	import { useChart, type TooltipSeries } from './chart-utils.js';
	import { getChartContext, Tooltip as TooltipPrimitive } from 'layerchart';
	import type { Snippet } from 'svelte';

	// Ported from the shadcn-svelte chart tooltip (written for layerchart 2.0.0-next) to the
	// layerchart 2.x stable API: tooltip state lives on the chart context, and the per-series
	// `payload` array is now `tooltip.series`.

	function defaultFormatter(value: any) {
		return `${value}`;
	}

	let {
		ref = $bindable(null),
		class: className,
		hideLabel = false,
		indicator = 'dot',
		hideIndicator = false,
		label,
		labelFormatter = defaultFormatter,
		labelClassName,
		formatter,
		nameKey,
		color,
		...restProps
	}: WithoutChildren<WithElementRef<HTMLAttributes<HTMLDivElement>>> & {
		hideLabel?: boolean;
		/** Fixed header label (or a chart config key). Defaults to the hovered x value. */
		label?: string;
		indicator?: 'line' | 'dot' | 'dashed';
		/** Chart config key to look up each item's label/icon, instead of the series key. */
		nameKey?: string;
		hideIndicator?: boolean;
		labelClassName?: string;
		labelFormatter?: ((value: any, series: TooltipSeries[]) => string | number | Snippet) | null;
		formatter?: Snippet<
			[
				{
					value: unknown;
					name: string;
					item: TooltipSeries;
					index: number;
					series: TooltipSeries[];
				}
			]
		>;
	} = $props();

	const chart = useChart();
	const chartCtx = getChartContext();

	const items = $derived(chartCtx.tooltip.series.filter((s) => s.visible));

	const formattedLabel = $derived.by(() => {
		const data = chartCtx.tooltip.data;
		if (hideLabel || data == null) return null;

		const value =
			typeof label === 'string'
				? (chart.config[label as keyof typeof chart.config]?.label ?? label)
				: chartCtx.x(data);

		if (value == null) return null;
		if (!labelFormatter) return value;
		return labelFormatter(value, items);
	});

	const nestLabel = $derived(items.length === 1 && indicator !== 'dot');
</script>

{#snippet TooltipLabel()}
	{#if formattedLabel}
		<div class={cn('font-medium', labelClassName)}>
			{#if typeof formattedLabel === 'function'}
				{@render formattedLabel()}
			{:else}
				{formattedLabel}
			{/if}
		</div>
	{/if}
{/snippet}

<TooltipPrimitive.Root variant="none">
	<div
		bind:this={ref}
		class={cn(
			'grid min-w-[9rem] items-start gap-1.5 rounded-lg border border-border/50 bg-background px-2.5 py-1.5 text-xs shadow-xl',
			className
		)}
		{...restProps}
	>
		{#if !nestLabel}
			{@render TooltipLabel()}
		{/if}
		<div class="grid gap-1.5">
			{#each items as item, i (item.key)}
				{@const itemConfig = chart.config[nameKey ?? item.key]}
				{@const indicatorColor = color || item.color}
				{@const name = itemConfig?.label || item.label}
				<div
					class={cn(
						'flex w-full flex-wrap items-stretch gap-2 [&>svg]:size-2.5 [&>svg]:text-muted-foreground',
						indicator === 'dot' && 'items-center'
					)}
				>
					{#if formatter && item.value !== undefined}
						{@render formatter({ value: item.value, name, item, index: i, series: items })}
					{:else}
						{#if itemConfig?.icon}
							<itemConfig.icon />
						{:else if !hideIndicator}
							<div
								style="--color-bg: {indicatorColor}; --color-border: {indicatorColor};"
								class={cn('shrink-0 rounded-[2px] border-(--color-border) bg-(--color-bg)', {
									'size-2.5': indicator === 'dot',
									'h-full w-1': indicator === 'line',
									'w-0 border-[1.5px] border-dashed bg-transparent': indicator === 'dashed',
									'my-0.5': nestLabel && indicator === 'dashed'
								})}
							></div>
						{/if}
						<div
							class={cn(
								'flex flex-1 shrink-0 justify-between leading-none',
								nestLabel ? 'items-end' : 'items-center'
							)}
						>
							<div class="grid gap-1.5">
								{#if nestLabel}
									{@render TooltipLabel()}
								{/if}
								<span class="text-muted-foreground">{name}</span>
							</div>
							{#if item.value !== undefined}
								<span class="font-mono font-medium text-foreground tabular-nums">
									{item.value.toLocaleString()}
								</span>
							{/if}
						</div>
					{/if}
				</div>
			{/each}
		</div>
	</div>
</TooltipPrimitive.Root>
