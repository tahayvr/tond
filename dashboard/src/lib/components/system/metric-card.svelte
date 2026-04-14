<script lang="ts">
	import type { Component } from 'svelte';

	interface Props {
		value: number | string;
		label: string;
		icon?: Component;
		detail: string;
		healthy?: boolean;
		healthyDetail?: string;
	}

	let { value, label, icon: Icon, detail, healthy, healthyDetail }: Props = $props();
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
		{value}
	</div>
	<p class="mt-1 font-mono text-[10px] text-muted-foreground/60">{detail}</p>
	{#if healthy !== undefined}
		<div class="mt-1 flex items-center gap-2">
			<span
				class="inline-block size-2 rounded-full {healthy ? 'bg-success' : 'bg-muted-foreground/40'}"
			></span>
			<span class="font-mono text-[10px] text-muted-foreground/60">{healthyDetail}</span>
		</div>
	{/if}
</div>
