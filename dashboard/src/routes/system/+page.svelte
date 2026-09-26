<script lang="ts">
	import type { SystemMetrics } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SystemStatsGrid from '$lib/components/system/system-stats-grid.svelte';
	import TondHealthPanel from '$lib/components/system/TondHealthPanel.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let metrics = $derived(data.metrics as SystemMetrics | null);
	let error = $state('');

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex h-14 items-center justify-between">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">System</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">HOST_NODE_STATUS</p>
		</div>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-destructive">
			{error}
		</div>
	{/if}

	{#if metrics}
		<SystemStatsGrid {metrics} />
	{/if}

	<TondHealthPanel />
</div>
