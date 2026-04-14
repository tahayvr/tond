<script lang="ts">
	import type { Component } from 'svelte';

	interface Props {
		value: number;
		label: string;
		icon?: Component;
		color?: string;
		format?: (val: number) => string;
		detail?: string;
	}

	let {
		value,
		label,
		icon: Icon,
		color = 'bg-success/60',
		format = (v) => `${v.toFixed(1)}%`,
		detail
	}: Props = $props();

	let width = $derived(Math.min(value, 100).toFixed(1));
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-2 flex items-center gap-2">
		{#if Icon}
			<Icon class="h-4 w-4 text-muted-foreground/70" />
		{/if}
		<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
			{label}
		</span>
	</div>
	<div class="font-mono text-3xl text-foreground tabular-nums">
		{format(value)}
	</div>
	<div class="mt-2 h-2 w-full overflow-hidden rounded-full bg-muted">
		<div class="h-2 rounded-full transition-all {color}" style="width: {width}%"></div>
	</div>
	{#if detail}
		<p class="mt-1 font-mono text-[10px] text-muted-foreground/60">
			{detail}
		</p>
	{/if}
</div>
