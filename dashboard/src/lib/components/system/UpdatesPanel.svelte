<!--
  UpdatesPanel — System → Updates: the running version, the latest release and
  its notes, "Update now" (a full snapshot first, automatic rollback on
  failure), the progress of a run across the server's restart, and "Roll back"
  for 7 days after an update.

  While a run is going the panel polls GET /api/system/update every 2 s: the
  server restarts in the middle, so failed requests are expected and shown as
  "restarting". Once an update succeeds the page reloads to pick up the new
  dashboard.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		checkForUpdates,
		getUpdateOverview,
		setUpdateCheck,
		startRollback,
		startUpdate,
		type UpdateOverview,
		type UpdateRun
	} from '$lib/api';
	import { errorMessage, useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import { relativeUnix } from '$lib/uptime';
	import {
		formatDate,
		isRestarting,
		needsReload,
		runSummary,
		stepStates,
		type Tone
	} from '$lib/updates';
	import { cn } from '$lib/utils';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import ReleaseNotes from './ReleaseNotes.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import RefreshIcon from '~icons/ri/refresh-line';
	import UpdateIcon from '~icons/ri/download-cloud-2-line';
	import HistoryIcon from '~icons/ri/history-line';
	import CheckIcon from '~icons/ri/checkbox-circle-fill';
	import SpinnerIcon from '~icons/ri/loader-4-line';
	import SkipIcon from '~icons/ri/skip-forward-line';
	import DotIcon from '~icons/ri/checkbox-blank-circle-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import InfoIcon from '~icons/ri/information-line';
	import WarningIcon from '~icons/ri/alert-line';
	import ErrorIcon from '~icons/ri/close-circle-line';

	let { canManage }: { canManage: boolean } = $props();

	let overview = $state<UpdateOverview | null>(null);
	let loadError = $state('');
	/** The server stopped answering during a run (it's restarting). */
	let restarting = $state(false);
	/** The version this page was loaded with (its dashboard files). */
	let loadedVersion = $state<string | null>(null);
	let reloading = $state(false);
	let now = $state(Date.now() / 1000);

	let run = $derived(overview?.run ?? null);
	let running = $derived(run?.effective_state === 'running');
	let latest = $derived(overview?.latest ?? null);
	let summary = $derived(run ? runSummary(run) : null);
	let showRun = $derived(
		run !== null && (running || now - (run.finished_at ?? run.updated_at) < 7 * 86_400)
	);

	async function load() {
		try {
			const o = await getUpdateOverview();
			overview = o;
			loadedVersion ??= o.current_version;
			restarting = false;
			loadError = '';
			now = Date.now() / 1000;
			if (needsReload(loadedVersion, o.current_version, o.run) && !reloading) {
				reloading = true;
				setTimeout(() => location.reload(), 2500);
			}
		} catch (e) {
			if (running && isRestarting(e)) {
				restarting = true;
			} else {
				loadError = `Couldn't load update information: ${errorMessage(e)}`;
			}
		}
	}

	$effect(() => {
		untrack(load);
	});

	// Poll only while a run is going.
	$effect(() => {
		if (!running) return;
		return pollWhileVisible(() => untrack(load), 2000, { immediate: false });
	});

	const check = useAction(() => checkForUpdates(), {
		onSuccess: (o) => {
			overview = o;
			now = Date.now() / 1000;
		}
	});
	const toggleCheck = useAction((enabled: boolean) => setUpdateCheck(enabled), {
		onSuccess: (o) => (overview = o)
	});

	function startedRun(r: UpdateRun) {
		if (!overview) return;
		overview = {
			...overview,
			run: { ...r, effective_state: r.state, log: r.log ?? [] },
			rollback: null
		};
	}

	async function handleUpdate() {
		if (!latest) return;
		await confirmAction({
			title: `Update Tond to ${latest.version}?`,
			description: `Tond ${overview?.current_version} → ${latest.version}. This takes a few minutes; you can stay on this page.`,
			details: [
				'A full snapshot is taken first (it appears under Snapshots).',
				'Tond stops for about a minute to install the new version and migrate its database; the dashboard reconnects by itself.',
				'Your apps keep running. They are unreachable for a few seconds while the proxy restarts.',
				'If anything fails, the current version (and its database) is put back automatically.',
				'You can roll back to the current version for 7 days.'
			],
			destructive: false,
			confirmLabel: `Update to ${latest.version}`,
			pendingLabel: 'Starting…',
			onconfirm: async () => startedRun(await startUpdate({ version: latest!.version }))
		});
	}

	async function handleRollback() {
		const rb = overview?.rollback;
		if (!rb) return;
		const details = [
			'A full snapshot of the current state is taken first.',
			`Tond stops for about a minute and starts again as ${rb.to_version}; apps are unreachable for a few seconds.`
		];
		if (rb.restores_database) {
			details.push(
				`The update to ${overview?.current_version} changed the database, so the database from before it (${formatDate(rb.backup_created_at)}) comes back too: apps, deploys, settings and users changed since then are lost. The snapshot keeps them.`
			);
		} else {
			details.push('The database stays as it is: the update did not change it.');
		}
		await confirmAction({
			title: `Roll back to Tond ${rb.to_version}?`,
			description: `Undo the update to ${overview?.current_version}.`,
			details,
			destructive: rb.restores_database,
			confirmLabel: `Roll back to ${rb.to_version}`,
			pendingLabel: 'Starting…',
			onconfirm: async () => startedRun(await startRollback())
		});
	}

	const TONE_BOX: Record<Tone, string> = {
		info: 'border-info/40 bg-info/10',
		success: 'border-success/40 bg-success/10',
		warning: 'border-warn/40 bg-warn/10',
		danger: 'border-destructive/40 bg-destructive/10'
	};
	const TONE_TEXT: Record<Tone, string> = {
		info: 'text-info',
		success: 'text-success',
		warning: 'text-warn',
		danger: 'text-destructive'
	};
</script>

<div class="space-y-6">
	<InlineError message={loadError} ondismiss={() => (loadError = '')} />

	<Card.Root>
		<Card.Header>
			<div class="flex flex-wrap items-start justify-between gap-3">
				<div class="space-y-1.5">
					<Card.Title class="flex items-center gap-2">
						<UpdateIcon class="size-5 text-muted-foreground" aria-hidden="true" />
						Tond version
					</Card.Title>
					<Card.Description>
						{#if overview?.check.checked_at}
							Checked for new releases {relativeUnix(overview.check.checked_at, now)}{overview.check
								.enabled
								? ' · every day'
								: ''}
						{:else if overview && !overview.check.enabled}
							The daily check for new releases is off.
						{:else}
							New releases are looked up once a day.
						{/if}
					</Card.Description>
				</div>
				{#if canManage}
					<Button
						size="sm"
						variant="outline"
						onclick={() => check.run()}
						disabled={check.pending || !overview}
					>
						<RefreshIcon class={cn('size-4', check.pending && 'animate-spin')} aria-hidden="true" />
						{check.pending ? 'Checking…' : 'Check now'}
					</Button>
				{/if}
			</div>
		</Card.Header>
		<Card.Content class="space-y-5">
			<InlineError
				message={check.error || toggleCheck.error}
				ondismiss={() => (check.clear(), toggleCheck.clear())}
			/>
			{#if !overview}
				<div class="space-y-2" aria-busy="true">
					<Skeleton class="h-8 w-48" />
					<Skeleton class="h-4 w-72" />
				</div>
			{:else}
				<div class="flex flex-wrap items-end justify-between gap-4">
					<div class="space-y-1">
						<p class="text-sm text-muted-foreground">Running</p>
						<p class="text-2xl font-bold tabular-nums" data-testid="current-version">
							Tond {overview.current_version}
						</p>
						{#if overview.update_available && latest}
							<p class="flex items-center gap-1.5 text-sm text-info" data-testid="update-available">
								<InfoIcon class="size-4" aria-hidden="true" />
								Tond {latest.version} is available{latest.published_at
									? ` (released ${formatDate(latest.published_at)})`
									: ''}
							</p>
						{:else if latest}
							<p class="flex items-center gap-1.5 text-sm text-success">
								<CheckIcon class="size-4" aria-hidden="true" />
								Up to date
							</p>
						{:else}
							<p class="text-sm text-muted-foreground">
								No release information yet{overview.check.enabled
									? ''
									: ' (the daily check is off)'}.
							</p>
						{/if}
					</div>
					{#if canManage && overview.update_available && latest}
						<Button
							onclick={handleUpdate}
							disabled={running || Boolean(overview.unsupported_reason)}
							data-testid="update-now"
						>
							<UpdateIcon class="size-4" aria-hidden="true" />
							{running ? 'Update running…' : `Update to ${latest.version}`}
						</Button>
					{/if}
				</div>

				{#if overview.check.error}
					<p class="text-sm text-warn">
						The last check didn't work: {overview.check.error}. It tries again tomorrow; "Check now"
						tries right away.
					</p>
				{/if}
				{#if overview.unsupported_reason && overview.update_available}
					<p class="text-sm text-muted-foreground">{overview.unsupported_reason}</p>
				{/if}

				{#if canManage}
					<div class="flex items-start gap-3 border-t pt-4">
						<Checkbox
							id="update-check-enabled"
							checked={overview.check.enabled}
							disabled={toggleCheck.pending}
							onCheckedChange={(v) => toggleCheck.run(v === true)}
						/>
						<div class="grid gap-1">
							<Label for="update-check-enabled">Check for new releases every day</Label>
							<p class="text-xs text-muted-foreground">
								Tond asks GitHub for the list of releases once a day. Nothing about this server is
								sent. Updates are never installed without you.
							</p>
						</div>
					</div>
				{/if}
			{/if}
		</Card.Content>
	</Card.Root>

	{#if run && summary && showRun}
		<Card.Root data-testid="update-run">
			<Card.Header>
				<Card.Title>{run.kind === 'rollback' ? 'Rollback' : 'Update'}</Card.Title>
				<Card.Description>
					Started {relativeUnix(run.started_at, now)}{run.actor ? ` by ${run.actor}` : ''}
				</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-4">
				<div
					class={cn('flex items-start gap-3 rounded-md border p-3', TONE_BOX[summary.tone])}
					role={running ? 'status' : undefined}
					aria-live="polite"
				>
					{#if running}
						<SpinnerIcon
							class={cn('mt-0.5 size-4 shrink-0 animate-spin', TONE_TEXT[summary.tone])}
							aria-hidden="true"
						/>
					{:else if summary.tone === 'success'}
						<CheckIcon class="mt-0.5 size-4 shrink-0 text-success" aria-hidden="true" />
					{:else if summary.tone === 'warning'}
						<WarningIcon class="mt-0.5 size-4 shrink-0 text-warn" aria-hidden="true" />
					{:else}
						<ErrorIcon class="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" />
					{/if}
					<div class="min-w-0 space-y-1 text-sm">
						<p class="font-medium" data-testid="run-title">{summary.title}</p>
						{#if restarting}
							<p class="text-muted-foreground">
								Tond is restarting. This page reconnects by itself; there's no need to reload.
							</p>
						{:else if reloading}
							<p class="text-muted-foreground">Loading the new dashboard…</p>
						{:else if summary.detail}
							<p class="break-words text-muted-foreground">{summary.detail}</p>
						{/if}
					</div>
				</div>

				{#if running && run.step !== 'rollback'}
					<ol class="grid gap-2 text-sm sm:grid-cols-3 lg:grid-cols-6" aria-label="Steps">
						{#each stepStates(run) as { step, state } (step.id)}
							<li
								class={cn(
									'flex items-center gap-2 rounded-sm border px-2.5 py-1.5',
									state === 'current' && 'border-info/40 bg-info/10 text-info',
									state === 'done' && 'text-foreground',
									(state === 'pending' || state === 'skipped') && 'text-muted-foreground'
								)}
								aria-current={state === 'current' ? 'step' : undefined}
							>
								{#if state === 'done'}
									<CheckIcon class="size-4 shrink-0 text-success" aria-hidden="true" />
								{:else if state === 'current'}
									<SpinnerIcon class="size-4 shrink-0 animate-spin" aria-hidden="true" />
								{:else if state === 'skipped'}
									<SkipIcon class="size-4 shrink-0" aria-hidden="true" />
								{:else}
									<DotIcon class="size-4 shrink-0" aria-hidden="true" />
								{/if}
								<span class="truncate">{step.label}</span>
								<span class="sr-only">({state})</span>
							</li>
						{/each}
					</ol>
				{/if}

				{#if run.log.length > 0}
					<details class="group rounded-md border" open={running || summary.tone !== 'success'}>
						<summary class="cursor-pointer px-3 py-2 text-sm text-muted-foreground select-none">
							Output ({run.log.length} line{run.log.length === 1 ? '' : 's'})
						</summary>
						<pre
							class="max-h-80 overflow-auto border-t bg-log-bg px-3 py-2 text-xs leading-relaxed text-log-fg"
							data-testid="run-log">{run.log.join('\n')}</pre>
					</details>
				{/if}
				{#if run.snapshot_id}
					<p class="text-xs text-muted-foreground">
						Snapshot taken before it: <a class="underline underline-offset-2" href="/snapshots"
							>{run.snapshot_id.slice(0, 8)}</a
						>
					</p>
				{/if}
			</Card.Content>
		</Card.Root>
	{/if}

	{#if overview?.rollback && !running}
		{@const rb = overview.rollback}
		<Card.Root data-testid="rollback-card">
			<Card.Header>
				<div class="flex flex-wrap items-start justify-between gap-3">
					<div class="space-y-1.5">
						<Card.Title class="flex items-center gap-2">
							<HistoryIcon class="size-5 text-muted-foreground" aria-hidden="true" />
							Roll back to {rb.to_version}
						</Card.Title>
						<Card.Description>
							Tond {overview.current_version} was installed on {formatDate(rb.backup_created_at)}.
							You can go back to {rb.to_version} until {formatDate(rb.available_until)}.
						</Card.Description>
					</div>
					{#if canManage}
						<Button
							variant="outline"
							size="sm"
							onclick={handleRollback}
							disabled={Boolean(overview.unsupported_reason)}
						>
							<HistoryIcon class="size-4" aria-hidden="true" />
							Roll back
						</Button>
					{/if}
				</div>
			</Card.Header>
			{#if rb.restores_database}
				<Card.Content>
					<p class="flex items-start gap-2 text-sm text-warn">
						<WarningIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
						The update changed the database, so rolling back also restores the database from before it:
						changes made since {formatDate(rb.backup_created_at)} are lost (a snapshot keeps them).
					</p>
				</Card.Content>
			{/if}
		</Card.Root>
	{/if}

	{#if overview && overview.changelog.length > 0}
		<Card.Root>
			<Card.Header>
				<Card.Title>What's new</Card.Title>
				<Card.Description>
					{overview.changelog.length === 1
						? `Changes in ${overview.changelog[0].version}`
						: `Changes in the ${overview.changelog.length} releases since ${overview.current_version}`}
				</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-6">
				{#each overview.changelog as release (release.version)}
					<section class="space-y-2" aria-labelledby="release-{release.version}">
						<div class="flex flex-wrap items-baseline justify-between gap-2 border-b pb-1.5">
							<h3 id="release-{release.version}" class="font-semibold">{release.name}</h3>
							<span class="flex items-center gap-3 text-xs text-muted-foreground">
								{formatDate(release.published_at)}
								{#if release.url}
									<a
										href={release.url}
										target="_blank"
										rel="noopener noreferrer"
										class="inline-flex items-center gap-1 underline underline-offset-2 hover:no-underline"
									>
										On GitHub <ExternalIcon class="size-3" aria-hidden="true" />
									</a>
								{/if}
							</span>
						</div>
						<ReleaseNotes markdown={release.body} />
					</section>
				{/each}
			</Card.Content>
		</Card.Root>
	{/if}
</div>
