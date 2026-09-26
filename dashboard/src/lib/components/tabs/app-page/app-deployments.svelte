<script lang="ts">
	// Deployment history: what ran (commit or image), who triggered it, how long
	// it took and why it failed. Each row opens its deploy log; "Roll back to
	// this" restores a deployment's exact image after a confirmation that names
	// the target and what stays as it is.
	import { Button } from '$lib/components/ui/button/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import type { Deployment } from '$lib/api';
	import {
		deploymentSubject,
		formatDuration,
		relativeTime,
		shortSha,
		statusView,
		triggerLabel
	} from '$lib/deploy-view';
	import { cn } from '$lib/utils';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import HistoryIcon from '~icons/ri/history-line';
	import DoneIcon from '~icons/ri/checkbox-circle-fill';
	import FailedIcon from '~icons/ri/close-circle-fill';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import WaitIcon from '~icons/ri/hourglass-line';
	import NeutralIcon from '~icons/ri/indeterminate-circle-line';
	import ChevronIcon from '~icons/ri/arrow-right-s-line';

	interface Props {
		appId: string;
		appName?: string;
		deployments: Deployment[];
		loading: boolean;
		error: string;
		/** Restore a deployment's exact image */
		onRollback: (deploymentId: string) => void | Promise<void>;
		onViewGeneratedDockerfile: (deploymentId: string) => Promise<string>;
	}

	let {
		appId,
		appName = 'this app',
		deployments,
		loading,
		error,
		onRollback,
		onViewGeneratedDockerfile
	}: Props = $props();
	let dockerfileDialogOpen = $state(false);
	let dockerfileLoading = $state(false);
	let dockerfileError = $state('');
	let dockerfileContent = $state('');
	let selectedDeploymentId = $state('');

	// Rollback confirmation
	let confirmTarget = $state<Deployment | null>(null);
	let confirmOpen = $state(false);
	let restoring = $state(false);

	/** What "Roll back to previous" restores (the server picks the same one). */
	let previousTarget = $derived(deployments.find((d) => d.rollback_available) ?? null);

	function handleAskRestore(d: Deployment) {
		confirmTarget = d;
		confirmOpen = true;
	}

	async function handleConfirmRestore() {
		if (!confirmTarget) return;
		restoring = true;
		try {
			await onRollback(confirmTarget.id);
			confirmOpen = false;
		} finally {
			restoring = false;
		}
	}

	async function handleViewGeneratedDockerfile(deploymentId: string) {
		dockerfileDialogOpen = true;
		dockerfileLoading = true;
		dockerfileError = '';
		dockerfileContent = '';
		selectedDeploymentId = deploymentId;
		try {
			dockerfileContent = await onViewGeneratedDockerfile(deploymentId);
		} catch (e: any) {
			dockerfileError = e.message ?? 'Failed to load generated Dockerfile';
		} finally {
			dockerfileLoading = false;
		}
	}
</script>

<div class="space-y-4 pt-4">
	{#if error}
		<ErrorAlert title="Rollback failed" description={error} />
	{/if}

	<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
		<div class="space-y-1">
			<p class="text-sm text-muted-foreground">
				The last 20 deployments. Open one to see its steps and log.
			</p>
			<p class="text-xs text-muted-foreground/80">
				Images of the last 3 successful deployments stay on the server for rollback; older ones are
				pruned to save disk space.
			</p>
		</div>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<span {...props} class="shrink-0">
						<Button
							variant="outline"
							size="sm"
							disabled={!previousTarget}
							onclick={() => previousTarget && handleAskRestore(previousTarget)}
						>
							<HistoryIcon class="size-4" />
							Roll back to previous
						</Button>
					</span>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				{#if previousTarget}
					<p>Restore {deploymentSubject(previousTarget)}</p>
				{:else}
					<p>No earlier deployment with an image still on this server</p>
				{/if}
			</Tooltip.Content>
		</Tooltip.Root>
	</div>

	<Dialog.Root bind:open={confirmOpen}>
		<Dialog.Content class="max-w-lg">
			<Dialog.Header>
				<Dialog.Title>Roll back {appName}?</Dialog.Title>
				<Dialog.Description>
					Tond runs the exact image of this deployment again and switches traffic once it answers.
				</Dialog.Description>
			</Dialog.Header>
			{#if confirmTarget}
				<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 rounded-md border p-3 text-sm">
					{#if confirmTarget.commit_sha}
						<dt class="text-muted-foreground">Commit</dt>
						<dd class="min-w-0">
							<code class="text-xs">{shortSha(confirmTarget.commit_sha)}</code>
							<span class="break-words">{confirmTarget.commit_message ?? ''}</span>
						</dd>
					{/if}
					<dt class="text-muted-foreground">Image</dt>
					<dd class="min-w-0 font-mono text-xs break-all">{confirmTarget.image_tag ?? '—'}</dd>
					<dt class="text-muted-foreground">Deployed</dt>
					<dd>
						{new Date(confirmTarget.started_at).toLocaleString()}
						<span class="text-muted-foreground">({relativeTime(confirmTarget.started_at)})</span>
					</dd>
				</dl>
			{/if}
			<p class="text-sm text-muted-foreground">
				Kept as they are now: environment variables, volumes and their data, the domain and resource
				limits. Only the code and its dependencies go back.
			</p>
			<Dialog.Footer>
				<Button variant="outline" onclick={() => (confirmOpen = false)} disabled={restoring}>
					Cancel
				</Button>
				<Button onclick={handleConfirmRestore} disabled={restoring}>
					{restoring ? 'Starting rollback…' : 'Roll back'}
				</Button>
			</Dialog.Footer>
		</Dialog.Content>
	</Dialog.Root>

	<Dialog.Root bind:open={dockerfileDialogOpen}>
		<Dialog.Content class="max-w-4xl">
			<Dialog.Header>
				<Dialog.Title>Generated Dockerfile</Dialog.Title>
				<Dialog.Description>
					The Dockerfile deployment {shortSha(selectedDeploymentId)} was built with.
				</Dialog.Description>
			</Dialog.Header>
			{#if dockerfileError}
				<ErrorAlert title="Error" description={dockerfileError} />
			{:else if dockerfileLoading}
				<div class="py-8 text-center text-sm text-muted-foreground">Loading Dockerfile...</div>
			{:else}
				<Textarea value={dockerfileContent} readonly class="min-h-96 font-mono text-xs" />
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	{#if loading && deployments.length === 0}
		<div class="py-8 text-center text-muted-foreground">Loading...</div>
	{:else if deployments.length === 0}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No deployments yet</p>
			<p class="mt-1 text-sm text-muted-foreground/60">Deploy this app to see its history here</p>
		</div>
	{:else}
		<ul class="divide-y rounded-md border" aria-label="Deployments">
			{#each deployments as d (d.id)}
				{@const view = statusView(d.status, d.queue_position)}
				<li class="flex flex-col gap-2 p-3 sm:flex-row sm:items-center sm:gap-4">
					<a
						href="/apps/{appId}/deployments/{d.id}"
						class="group flex min-w-0 flex-1 items-start gap-3 rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
						aria-label="Open deployment {shortSha(d.id)}: {view.label}"
					>
						<span class="mt-0.5 shrink-0" title={view.label}>
							{#if view.tone === 'success'}
								<DoneIcon class="size-5 text-success" />
							{:else if view.tone === 'failure'}
								<FailedIcon class="size-5 text-destructive" />
							{:else if view.tone === 'running'}
								<SpinnerIcon class="size-5 animate-spin" />
							{:else if view.tone === 'waiting'}
								<WaitIcon class="size-5 text-muted-foreground" />
							{:else}
								<NeutralIcon class="size-5 text-muted-foreground" />
							{/if}
						</span>
						<span class="min-w-0 flex-1 space-y-0.5">
							<span class="flex min-w-0 items-center gap-2">
								<span
									class={cn(
										'truncate text-sm font-medium group-hover:underline',
										!d.commit_sha && 'font-mono text-xs',
										d.image_pruned && !d.commit_sha && 'line-through opacity-60'
									)}>{deploymentSubject(d)}</span
								>
								{#if d.is_current}
									<Badge variant="outline" class="shrink-0 border-success/50 text-success"
										>current</Badge
									>
								{/if}
								{#if d.image_pruned}
									<span class="shrink-0 text-xs text-muted-foreground">(pruned)</span>
								{/if}
							</span>
							<span class="block truncate text-xs text-muted-foreground">
								{view.tone === 'success' || view.tone === 'failure'
									? ''
									: `${view.label} · `}{triggerLabel(d)} · {relativeTime(
									d.started_at
								)}{d.duration_secs != null ? ` · ${formatDuration(d.duration_secs)}` : ''}
							</span>
							{#if d.status === 'failed'}
								<span class="block truncate text-xs text-destructive">
									{d.failure?.title ?? 'Failed'}
								</span>
							{/if}
						</span>
						<ChevronIcon class="mt-0.5 hidden size-4 shrink-0 text-muted-foreground sm:block" />
					</a>
					<div class="flex shrink-0 items-center gap-2 pl-8 sm:pl-0">
						{#if d.has_generated_dockerfile}
							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-xs"
								onclick={() => handleViewGeneratedDockerfile(d.id)}
							>
								Dockerfile
							</Button>
						{/if}
						{#if d.rollback_available}
							<Button
								variant="outline"
								size="sm"
								class="h-7 text-xs"
								onclick={() => handleAskRestore(d)}
							>
								Roll back to this
							</Button>
						{:else if d.status === 'succeeded' && !d.is_current}
							<Tooltip.Root>
								<Tooltip.Trigger>
									{#snippet child({ props })}
										<span {...props} class="inline-block">
											<Button variant="outline" size="sm" class="h-7 text-xs" disabled>
												Roll back to this
											</Button>
										</span>
									{/snippet}
								</Tooltip.Trigger>
								<Tooltip.Content>
									<p>{d.rollback_unavailable_reason ?? 'Rollback unavailable'}</p>
								</Tooltip.Content>
							</Tooltip.Root>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</div>
