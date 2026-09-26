<script lang="ts">
	// A metric tile with a sparkline: label, current value, a tiny line of
	// the window's buckets. Nulls break the line. Decorative for screen
	// readers apart from the label, value and `summary`.
	import { sparkPath } from '$lib/metrics-format';
	import { cn } from '$lib/utils';

	interface Props {
		label: string;
		/** Formatted headline value. */
		value: string;
		values: (number | null)[];
		/** CSS colour (theme variable). */
		color?: string;
		/** Top of the scale (default: the largest value). */
		max?: number;
		/** Small text under the value. */
		hint?: string;
		/** Highlights the tile (e.g. errors above 1%). */
		warn?: boolean;
		class?: string;
	}

	let {
		label,
		value,
		values,
		color = 'var(--chart-1)',
		max,
		hint,
		warn = false,
		class: className
	}: Props = $props();

	const W = 100;
	const H = 28;
	let d = $derived(sparkPath(values, W, H, max));
</script>

<div
	class={cn('min-w-0 rounded-lg border p-3', warn && 'border-warn/50 bg-warn/5', className)}
	data-testid="sparkline"
>
	<p class="truncate text-xs text-muted-foreground">{label}</p>
	<p class="text-lg font-semibold tabular-nums">{value}</p>
	<svg
		viewBox="0 0 {W} {H}"
		preserveAspectRatio="none"
		class="mt-1 block h-7 w-full"
		aria-hidden="true"
	>
		{#if d}
			<path
				{d}
				fill="none"
				stroke={color}
				stroke-width="1.5"
				stroke-linejoin="round"
				stroke-linecap="round"
				vector-effect="non-scaling-stroke"
			/>
		{:else}
			<line
				x1="0"
				x2={W}
				y1={H - 1}
				y2={H - 1}
				class="stroke-border"
				stroke-width="1"
				vector-effect="non-scaling-stroke"
			/>
		{/if}
	</svg>
	{#if hint}
		<p class="truncate text-xs text-muted-foreground">{hint}</p>
	{/if}
</div>
