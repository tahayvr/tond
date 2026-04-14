<script lang="ts">
	import type { CronJob } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { goto } from '$app/navigation';
	import Clock from '@lucide/svelte/icons/clock';
	import Plus from '@lucide/svelte/icons/plus';
	import CronJobGrid from '$lib/components/cards/cron-job-card/cron-job-grid.svelte';
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
	<!-- Header -->
	<div class="flex h-14 items-center justify-between gap-4">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Cron Jobs</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{enabledCount}/{jobs.length} SCHEDULES ACTIVE
			</p>
		</div>
		<Button onclick={() => goto('/cron/new')}>
			<Plus class="mr-1 h-4 w-4" />
			New Cron Job
		</Button>
	</div>

	{#if jobs.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Clock class="mb-4 h-12 w-12 text-muted-foreground/40" />
			<p class="font-mono text-[11px] tracking-widest text-muted-foreground/60 uppercase">
				NO_SCHEDULES_FOUND
			</p>
			<p class="mt-2 text-sm text-muted-foreground/70">Create your first scheduled task</p>
			<Button class="mt-4" onclick={() => goto('/cron/new')}>
				<Plus class="mr-1 h-4 w-4" />
				New Cron Job
			</Button>
		</div>
	{:else}
		<CronJobGrid {jobs} />
	{/if}
</div>
