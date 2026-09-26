<script lang="ts">
	import type { CronJob } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { describeCron } from '$lib/cron-schedule';
	import { formatRelativePast, formatRelativeFuture } from '$lib/time';

	interface Props {
		job: CronJob;
	}

	let { job }: Props = $props();

	let schedule = $derived(describeCron(job.schedule));
</script>

<a href={`/cron/${job.id}`} class="block h-full rounded-sm" data-sveltekit-preload-data="hover">
	<Card.Root
		class="flex h-full w-full flex-col gap-3 rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="gap-2 p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Title class="min-w-0 truncate text-base">{job.name}</Card.Title>
				{#if !job.enabled}
					<StatusBadge status="paused" size="sm" />
				{:else}
					<StatusBadge status={job.status} kind="run" size="sm" />
				{/if}
			</div>
			<Card.Description class="text-xs">
				{schedule.ok ? schedule.text : job.schedule}{job.timezone ? ` (${job.timezone})` : ''}
			</Card.Description>
		</Card.Header>
		<Card.Content class="grid grow content-end gap-1.5 p-0 text-xs text-muted-foreground">
			<p class="truncate font-mono">{job.image}</p>
			<div class="flex flex-wrap items-center justify-between gap-2">
				<span class="flex items-center gap-1.5">
					Last: {formatRelativePast(job.last_run_at)}
					{#if job.last_run_status}
						<StatusBadge status={job.last_run_status} kind="run" size="sm" />
					{/if}
				</span>
				<span>Next: {job.enabled ? formatRelativeFuture(job.next_run_at) : '—'}</span>
			</div>
		</Card.Content>
	</Card.Root>
</a>
