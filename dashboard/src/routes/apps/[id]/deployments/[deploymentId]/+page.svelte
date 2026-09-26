<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	// Live deploy view: step progress, the streaming build/deploy log, elapsed
	// time and, once finished, the result with the likely cause of a failure.
	import { goto } from '$app/navigation';
	import { untrack } from 'svelte';
	import {
		deployApp,
		openDeploymentStream,
		type App,
		type Deployment,
		type DeploymentDetail,
		type DeploymentLogLine,
		type LogLine
	} from '$lib/api';
	import { createLogBuffer } from '$lib/log-buffer';
	import { pollWhileVisible } from '$lib/polling';
	import {
		formatDuration,
		isFinished,
		secondsBetween,
		shortSha,
		statusView,
		triggerLabel
	} from '$lib/deploy-view';
	import { setUiContext, clearUiContext, appContext } from '$lib/ui-context.svelte';
	import { cn } from '$lib/utils';
	import { can } from '$lib/auth.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';
	import DeploySteps from '$lib/components/deploy/deploy-steps.svelte';
	import DeployResult from '$lib/components/deploy/deploy-result.svelte';
	import DeploymentServices from '$lib/components/compose/deployment-services.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import TimerIcon from '~icons/ri/timer-line';
	import CommitIcon from '~icons/ri/git-commit-line';
	import UserIcon from '~icons/ri/user-line';
	import HourglassIcon from '~icons/ri/hourglass-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let app = $derived(data.app as App);
	let deployment = $state<Deployment>(untrack(() => data.deployment as DeploymentDetail));
	let logsTruncated = $state(untrack(() => (data.deployment as DeploymentDetail).logs_truncated));
	let lines = $state.raw<LogLine[]>([]);
	let connected = $state(false);
	let error = $state('');
	let redeploying = $state(false);
	let now = $state(Date.now());

	const buffer = createLogBuffer<LogLine>((next) => (lines = next));

	function toLine(l: DeploymentLogLine): LogLine {
		return { source: 'deploy', level: l.level, text: l.message, ts: l.ts };
	}

	let finished = $derived(isFinished(deployment.status));
	let view = $derived(statusView(deployment.status, deployment.queue_position));
	let elapsed = $derived(
		deployment.duration_secs ?? secondsBetween(deployment.started_at, deployment.finished_at, now)
	);
	let isRollback = $derived(deployment.trigger_source === 'rollback');

	// A new deployment id (navigating between deployments) resets the view.
	$effect(() => {
		const detail = data.deployment as DeploymentDetail;
		untrack(() => {
			deployment = detail;
			logsTruncated = detail.logs_truncated;
			buffer.replace(detail.logs.map(toLine));
		});
	});

	// Live stream while the deployment runs. The stream reconnects by itself
	// after a drop and every connection starts with a full snapshot, so a
	// result that landed while disconnected still shows up.
	$effect(() => {
		const appId = app.id;
		const deploymentId = (data.deployment as DeploymentDetail).id;
		if (untrack(() => isFinished(deployment.status))) return;
		const close = openDeploymentStream(appId, deploymentId, {
			onSnapshot(detail) {
				deployment = detail;
				logsTruncated = detail.logs_truncated;
				buffer.replace(detail.logs.map(toLine));
			},
			onLog(line) {
				buffer.push(toLine(line));
			},
			onDeployment(next) {
				deployment = next;
				if (isFinished(next.status)) {
					buffer.flush();
					connected = false;
				}
			},
			onOpen() {
				connected = true;
			},
			onDisconnect() {
				connected = false;
				buffer.flush();
			}
		});
		return close;
	});

	// Tick the elapsed time once a second while running (paused in hidden tabs).
	$effect(() => {
		if (finished) return;
		return pollWhileVisible(() => (now = Date.now()), 1000);
	});
	// The feed panel would open a second log socket for the same app.
	$effect(() => {
		const ctx = appContext(app);
		setUiContext({ ...ctx, feed: { ...ctx.feed, wsUrl: null } });
		return () => clearUiContext();
	});

	$effect(() => () => buffer.dispose());

	async function handleRedeploy() {
		redeploying = true;
		error = '';
		try {
			const started = await deployApp(app.id);
			await goto(`/apps/${app.id}/deployments/${started.deployment_id}`);
		} catch (e: any) {
			error = e.message ?? 'Could not start the deployment';
		} finally {
			redeploying = false;
		}
	}
</script>

<PageTitle title={`Deploy ${deployment.id.slice(0, 8)} · ${app.name}`} />

<div class="space-y-5">
	<Breadcrumb
		items={[
			{ label: 'Apps', href: '/apps' },
			{ label: app.name, href: `/apps/${app.id}` },
			{ label: 'Deployments', href: `/apps/${app.id}?tab=deployments` },
			{ label: shortSha(deployment.id) }
		]}
	/>

	<header class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
		<div class="min-w-0 space-y-2">
			<h2 class="flex flex-wrap items-center gap-2 text-2xl font-bold">
				<span class="truncate">
					{#if finished}
						{isRollback ? 'Rollback of' : 'Deployment of'} {app.name}
					{:else}
						{isRollback ? 'Rolling back' : 'Deploying'} {app.name}
					{/if}
				</span>
				<Badge
					variant={view.tone === 'failure' ? 'destructive' : 'outline'}
					class={cn(
						view.tone === 'success' && 'border-success/50 text-success',
						view.tone === 'running' && 'border-foreground/30'
					)}
				>
					{#if view.tone === 'running'}<SpinnerIcon class="animate-spin" />{/if}
					{#if view.tone === 'waiting'}<HourglassIcon />{/if}
					{view.label}
				</Badge>
			</h2>
			<div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-muted-foreground">
				<span class="flex items-center gap-1.5 tabular-nums" title="Elapsed time">
					<TimerIcon class="size-4" />
					{formatDuration(elapsed)}
				</span>
				<span class="flex items-center gap-1.5">
					<UserIcon class="size-4" />
					{triggerLabel(deployment)}
				</span>
				{#if deployment.commit_sha}
					<span class="flex min-w-0 items-center gap-1.5" title={deployment.commit_sha}>
						<CommitIcon class="size-4 shrink-0" />
						<code class="text-xs">{shortSha(deployment.commit_sha)}</code>
						{#if deployment.commit_message}
							<span class="max-w-[22rem] truncate">{deployment.commit_message}</span>
						{/if}
						{#if deployment.commit_author}
							<span class="hidden sm:inline">· {deployment.commit_author}</span>
						{/if}
					</span>
				{:else if deployment.image_tag}
					<code class="truncate text-xs" title="Image">{deployment.image_tag}</code>
				{/if}
			</div>
		</div>
	</header>

	{#if error}
		<p class="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive" role="alert">
			{error}
		</p>
	{/if}

	<section class="rounded-md border p-4" aria-label="Progress">
		{#if deployment.status === 'queued'}
			<p class="flex items-center gap-2 text-sm text-muted-foreground">
				<HourglassIcon class="size-4" />
				Waiting for the build queue{deployment.queue_position
					? ` (position ${deployment.queue_position})`
					: ''}. It starts as soon as the builds ahead of it finish.
			</p>
		{:else if (deployment.phases ?? []).length === 0}
			<p class="text-sm text-muted-foreground">
				This deployment predates step tracking; its log is below.
			</p>
		{:else}
			<DeploySteps phases={deployment.phases} {now} />
		{/if}
	</section>

	{#if (deployment.services ?? []).length > 0}
		<DeploymentServices services={deployment.services ?? []} />
	{/if}

	{#if finished}
		<DeployResult
			{deployment}
			appId={app.id}
			domain={app.domain}
			onRedeploy={can('apps:deploy') ? handleRedeploy : undefined}
			{redeploying}
		/>
	{/if}

	<section class="space-y-2" aria-label="Deploy log">
		<div class="flex items-center justify-between">
			<h3 class="text-sm font-medium">Log</h3>
			{#if logsTruncated}
				<span class="text-xs text-muted-foreground">Showing the newest 5,000 lines</span>
			{/if}
		</div>
		<LogViewer
			{lines}
			live={connected && !finished}
			downloadName="{app.name}-deploy-{shortSha(deployment.id)}"
			emptyText={finished ? 'This deployment wrote no log lines.' : 'Waiting for output…'}
			heightClass="h-[26rem] sm:h-[32rem]"
			showPrefix={false}
			label="Deploy log"
		/>
	</section>
</div>
