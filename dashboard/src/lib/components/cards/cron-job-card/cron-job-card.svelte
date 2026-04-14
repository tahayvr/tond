<script lang="ts">
	import { goto } from '$app/navigation';
	import { cn } from '$lib/utils';
	import type { CronJob } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';

	interface Props {
		job: CronJob;
	}

	let { job }: Props = $props();

	function statusDotClass(status: string, enabled: boolean): string {
		if (!enabled) return 'bg-muted-foreground/30';
		switch (status) {
			case 'running':
				return 'bg-warn animate-pulse';
			case 'failed':
				return 'bg-destructive';
			default:
				return 'bg-accent';
		}
	}

	function lastRunStatusClass(s: string | null): string {
		if (s === 'success') return 'text-success';
		if (s === 'failed') return 'text-destructive';
		return 'text-muted-foreground/40';
	}

	function formatRelative(dt: string | null): string {
		if (!dt) return '—';
		const diff = Date.now() - new Date(dt).getTime();
		const minutes = Math.floor(diff / 60_000);
		const hours = Math.floor(diff / 3_600_000);
		const days = Math.floor(diff / 86_400_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
	}

	function formatNextRun(dt: string | null): string {
		if (!dt) return '—';
		const diff = new Date(dt).getTime() - Date.now();
		const minutes = Math.floor(diff / 60_000);
		const hours = Math.floor(diff / 3_600_000);
		const days = Math.floor(diff / 86_400_000);
		if (diff < 0) return 'overdue';
		if (minutes < 1) return 'in <1m';
		if (minutes < 60) return `in ${minutes}m`;
		if (hours < 24) return `in ${hours}h`;
		return `in ${days}d`;
	}
</script>

<button
	type="button"
	class="block w-full text-left hover:cursor-pointer"
	onclick={() => goto(`/cron/${job.id}`)}
>
	<Card.Root
		class={cn(
			'flex w-full flex-col rounded-sm border-none p-5 transition-colors hover:border',
			!job.enabled && 'opacity-60'
		)}
	>
		<Card.Header class="p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Description class="text-xs uppercase">id: {job.id.slice(0, 8)}</Card.Description>
				<div class="flex items-center gap-2">
					{#if job.last_run_status}
						<span class="font-mono text-[10px] uppercase {lastRunStatusClass(job.last_run_status)}">
							{job.last_run_status}
						</span>
					{/if}
					<span
						class={cn(
							'font-mono text-[10px] uppercase',
							job.status === 'failed' ? 'text-destructive' : 'text-muted-foreground/50'
						)}
					>
						{job.enabled ? job.status : 'paused'}
					</span>
				</div>
			</div>
			<Card.Title class="text-base uppercase">{job.name}</Card.Title>
			<div class="flex items-center gap-2">
				<span
					class="inline-block size-2 shrink-0 rounded-full {statusDotClass(
						job.status,
						job.enabled
					)}"
				></span>
				<span class="font-mono text-[10px] tracking-wider text-secondary uppercase">
					{job.schedule}
				</span>
				{#if job.timezone}
					<span class="font-mono text-[10px] text-muted-foreground/50 uppercase">
						{job.timezone}
					</span>
				{/if}
			</div>
		</Card.Header>
		<Card.Content class="grow p-0">
			<div class="flex flex-col justify-center gap-2 py-4">
				<p class="text-center font-mono text-[10px] text-muted-foreground/70 uppercase">
					{job.image}
				</p>
				{#if job.concurrency_policy === 'allow'}
					<p class="text-center font-mono text-[10px] text-muted-foreground/50 uppercase">
						concurrent
					</p>
				{/if}
			</div>
		</Card.Content>
		<Card.Footer class="w-full justify-between p-0">
			<span class="font-mono text-[10px] text-muted-foreground/50">
				LAST: {formatRelative(job.last_run_at)}
			</span>
			<span class="font-mono text-[10px] text-muted-foreground/50">
				NEXT: {formatNextRun(job.next_run_at)}
			</span>
		</Card.Footer>
	</Card.Root>
</button>
