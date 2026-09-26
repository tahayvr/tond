<script lang="ts">
	// Step progress for a deployment: clone, build, start, health, release.
	import type { PhaseRecord } from '$lib/api';
	import { formatDuration, phaseLabel, secondsBetween, visiblePhases } from '$lib/deploy-view';
	import { cn } from '$lib/utils';
	import DoneIcon from '~icons/ri/checkbox-circle-fill';
	import FailedIcon from '~icons/ri/close-circle-fill';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import PendingIcon from '~icons/ri/checkbox-blank-circle-line';

	interface Props {
		phases: PhaseRecord[] | undefined;
		/** Current time in ms, ticking while the deployment runs. */
		now: number;
	}

	let { phases, now }: Props = $props();
	let steps = $derived(visiblePhases(phases));
</script>

{#if steps.length > 0}
	<ol
		class="grid gap-2 sm:grid-cols-[repeat(var(--steps),minmax(0,1fr))] sm:gap-0"
		style="--steps: {steps.length}"
		aria-label="Deploy steps"
	>
		{#each steps as step, i (step.phase)}
			{@const secs = secondsBetween(step.started_at, step.finished_at, now)}
			<li
				class="relative flex items-center gap-3 sm:flex-col sm:items-start sm:gap-2 sm:pr-4"
				aria-current={step.status === 'running' ? 'step' : undefined}
			>
				<div class="flex items-center gap-2 sm:w-full">
					<span
						class={cn(
							'flex size-6 shrink-0 items-center justify-center',
							step.status === 'succeeded' && 'text-success',
							step.status === 'failed' && 'text-destructive',
							step.status === 'running' && 'text-foreground',
							step.status === 'pending' && 'text-muted-foreground/60'
						)}
					>
						{#if step.status === 'succeeded'}
							<DoneIcon class="size-5" />
						{:else if step.status === 'failed'}
							<FailedIcon class="size-5" />
						{:else if step.status === 'running'}
							<SpinnerIcon class="size-5 animate-spin" />
						{:else}
							<PendingIcon class="size-5" />
						{/if}
					</span>
					{#if i < steps.length - 1}
						<span
							class={cn(
								'hidden h-px flex-1 sm:block',
								step.status === 'succeeded' ? 'bg-success/60' : 'bg-border'
							)}
						></span>
					{/if}
				</div>
				<div class="min-w-0">
					<p
						class={cn(
							'text-sm leading-tight',
							step.status === 'pending' ? 'text-muted-foreground' : 'font-medium'
						)}
					>
						{phaseLabel(step)}
					</p>
					<p class="text-xs text-muted-foreground tabular-nums">
						{#if step.status === 'running'}
							{formatDuration(secs)}
						{:else if step.status === 'succeeded' || step.status === 'failed'}
							{step.status === 'failed' ? 'Failed after ' : ''}{formatDuration(secs)}
						{:else}
							Waiting
						{/if}
					</p>
				</div>
			</li>
		{/each}
	</ol>
{/if}
