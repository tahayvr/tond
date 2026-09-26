<script lang="ts">
	// The outcome of a finished deployment: how long it took and where the app
	// is, or the likely cause of the failure with the lines that show it and a
	// link to where it's fixed.
	import type { Deployment } from '$lib/api';
	import { failureFix, formatDuration, statusView } from '$lib/deploy-view';
	import { safeUrl } from '$lib/utils';
	import { appTabHref } from '$lib/tabs';
	import { Button } from '$lib/components/ui/button/index.js';
	import DoneIcon from '~icons/ri/checkbox-circle-fill';
	import FailedIcon from '~icons/ri/close-circle-fill';
	import InfoIcon from '~icons/ri/information-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import RocketIcon from '~icons/ri/rocket-line';
	import ArrowRightIcon from '~icons/ri/arrow-right-line';

	interface Props {
		deployment: Deployment;
		appId: string;
		/** The app's domain, for the success link */
		domain?: string | null;
		onRedeploy?: () => void;
		redeploying?: boolean;
	}

	let { deployment, appId, domain = null, onRedeploy, redeploying = false }: Props = $props();

	let failure = $derived(deployment.failure ?? null);
	let fix = $derived(failureFix(failure));
	let appUrl = $derived(domain ? safeUrl(`https://${domain}`) : null);
	let isRollback = $derived(deployment.trigger_source === 'rollback');
</script>

{#if deployment.status === 'succeeded'}
	<section
		class="flex flex-col gap-3 rounded-md border border-success/40 bg-success/5 p-4 sm:flex-row sm:items-center"
		aria-label="Deploy result"
	>
		<DoneIcon class="size-6 shrink-0 text-success" />
		<div class="min-w-0 flex-1">
			<p class="font-medium">
				{isRollback ? 'Rolled back' : 'Deployed'} in {formatDuration(deployment.duration_secs)}
			</p>
			<p class="text-sm text-muted-foreground">
				{#if deployment.is_current}
					This version is live{appUrl ? ' at' : '.'}
					{#if appUrl}
						<a href={appUrl} target="_blank" rel="noreferrer" class="underline underline-offset-2"
							>{domain}</a
						>{/if}
				{:else}
					A newer deployment has replaced this one since.
				{/if}
			</p>
		</div>
		<div class="flex gap-2">
			{#if appUrl && deployment.is_current}
				<Button href={appUrl} target="_blank" rel="noreferrer" variant="outline" size="sm">
					<ExternalIcon /> Open app
				</Button>
			{/if}
			<Button href="/apps/{appId}" variant="ghost" size="sm">Back to app</Button>
		</div>
	</section>
{:else if deployment.status === 'failed'}
	<section
		class="space-y-3 rounded-md border border-destructive/40 bg-destructive/5 p-4"
		aria-label="Deploy result"
	>
		<div class="flex items-start gap-3">
			<FailedIcon class="mt-0.5 size-6 shrink-0 text-destructive" />
			<div class="min-w-0 flex-1 space-y-1">
				<p class="font-medium">{failure?.title ?? 'The deployment failed'}</p>
				{#if failure?.hint}
					<p class="text-sm text-muted-foreground">{failure.hint}</p>
				{/if}
				{#if deployment.finished_at}
					<p class="text-xs text-muted-foreground">
						Stopped after {formatDuration(deployment.duration_secs)}. The previous version, if any,
						keeps serving.
					</p>
				{/if}
			</div>
		</div>
		{#if failure?.excerpt?.length}
			<pre
				class="max-h-64 overflow-auto rounded-sm border bg-log-bg p-3 font-mono text-xs leading-5 text-log-fg"
				aria-label="Lines that show the problem">{failure.excerpt.join('\n')}</pre>
		{/if}
		{#if failure?.detail && failure.detail !== failure.title}
			<details class="text-xs text-muted-foreground">
				<summary class="cursor-pointer select-none">Error details</summary>
				<p class="mt-1 font-mono break-words whitespace-pre-wrap">{failure.detail}</p>
			</details>
		{/if}
		<div class="flex flex-wrap gap-2">
			{#if fix}
				<Button href={appTabHref(appId, 'settings', fix.section)} variant="outline" size="sm">
					{fix.label}
					<ArrowRightIcon />
				</Button>
			{/if}
			{#if onRedeploy}
				<Button size="sm" onclick={onRedeploy} disabled={redeploying}>
					<RocketIcon />
					{redeploying ? 'Starting…' : 'Deploy again'}
				</Button>
			{/if}
		</div>
	</section>
{:else}
	{@const view = statusView(deployment.status, deployment.queue_position)}
	<section
		class="flex items-center gap-3 rounded-md border bg-muted/30 p-4 text-sm"
		aria-label="Deploy result"
	>
		<InfoIcon class="size-5 shrink-0 text-muted-foreground" />
		<p>{view.label}</p>
	</section>
{/if}
