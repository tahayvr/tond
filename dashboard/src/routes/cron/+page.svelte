<script lang="ts">
	import type { CronJob } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import Clock from '~icons/ri/time-line';
	import Plus from '~icons/ri/add-line';
	import CronJobGrid from '$lib/components/cards/cron-job-card/cron-job-grid.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let jobs = $derived(data.jobs as CronJob[]);
	let enabledCount = $derived(jobs.filter((j) => j.enabled).length);

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="Cron jobs"
		description={jobs.length ? `${enabledCount} of ${jobs.length} enabled` : undefined}
	>
		{#snippet actions()}
			<Button href="/cron/new">
				<Plus class="mr-1 h-4 w-4" />
				New cron job
			</Button>
		{/snippet}
	</PageHeader>

	{#if jobs.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card px-4 py-16 text-center"
		>
			<Clock aria-hidden="true" class="mb-4 h-12 w-12 text-muted-foreground" />
			<p class="font-medium">No cron jobs yet</p>
			<p class="mt-2 text-sm text-muted-foreground">
				Run a container on a schedule: backups, reports, cleanup tasks.
			</p>
			<Button class="mt-4" href="/cron/new">
				<Plus class="mr-1 h-4 w-4" />
				New cron job
			</Button>
		</div>
	{:else}
		<CronJobGrid {jobs} />
	{/if}
</div>
