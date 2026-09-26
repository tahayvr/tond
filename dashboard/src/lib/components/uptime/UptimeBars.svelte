<!--
  UptimeBars — a status strip: one bar per period, green when every check
  passed, amber when a few failed, red when most did, grey without checks.
  Hover (or focus) a bar for its numbers; screen readers get a summary.
-->
<script lang="ts">
	import type { UptimeBar } from '$lib/api';
	import { BAR_CLASSES, barTitle, barTone, barsSummary } from '$lib/uptime';
	import { cn } from '$lib/utils';

	interface Props {
		bars: UptimeBar[];
		/** What one bar covers, for the tooltips. */
		width?: 'half-hour' | 'day';
		/** Accessible name, e.g. "Last 24 hours". */
		label: string;
		/** Bar height in Tailwind units. */
		class?: string;
	}

	let { bars, width = 'half-hour', label, class: className }: Props = $props();

	let summary = $derived(barsSummary(bars));
</script>

<div
	class={cn('flex h-8 items-stretch gap-px sm:gap-0.5', className)}
	role="img"
	aria-label="{label}: {summary}"
>
	{#each bars as bar (bar.t)}
		<span
			class={cn('min-w-0 flex-1 rounded-[2px]', BAR_CLASSES[barTone(bar)])}
			title={barTitle(bar, width)}
			data-tone={barTone(bar)}
		></span>
	{/each}
</div>
