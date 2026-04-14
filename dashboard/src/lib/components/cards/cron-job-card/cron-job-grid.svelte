<script lang="ts">
	import CronJobCard from './cron-job-card.svelte';
	import type { CronJob } from '$lib/api';

	interface Props {
		jobs: CronJob[];
	}

	let { jobs }: Props = $props();
</script>

<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
	{#each jobs as job (job.id)}
		<CronJobCard {job} />
	{/each}
</div>
