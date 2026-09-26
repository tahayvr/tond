<script lang="ts">
	import type { SystemMetrics } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SystemStatsGrid from '$lib/components/system/system-stats-grid.svelte';
	import TondHealthPanel from '$lib/components/system/TondHealthPanel.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
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
	<PageHeader title="System" description="Host resources and Tond's own health." />

	<InlineError message={error} ondismiss={() => (error = '')} />

	{#if metrics}
		<SystemStatsGrid {metrics} />
	{/if}

	<TondHealthPanel />
</div>
