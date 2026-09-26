<script lang="ts">
	import { can } from '$lib/auth.svelte';
	import {
		createSnapshot,
		deleteSnapshot,
		restoreSnapshot,
		rollbackAppToSnapshot,
		listStorageProviders,
		getBackupConfig,
		updateBackupConfig,
		type Snapshot,
		type StorageProvider
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Badge from '$lib/components/ui/badge/badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { useAction } from '$lib/action.svelte';
	import { invalidateAll } from '$app/navigation';
	import { setUiContext, clearUiContext, snapshotsContext } from '$lib/ui-context.svelte';
	import Trash2 from '~icons/ri/delete-bin-line';
	import RefreshCw from '~icons/ri/refresh-line';
	import Settings2 from '~icons/ri/equalizer-line';
	import Plus from '~icons/ri/add-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import CloudIcon from '~icons/ri/cloud-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	type SnapshotsPageData = {
		snapshots: Snapshot[];
	};

	// ── State ─────────────────────────────────────────────────────────────────

	let snapshots = $derived((data as SnapshotsPageData).snapshots);
	let error = $state('');
	let successMessage = $state('');

	// Config panel
	let showConfig = $state(false);
	let configLoading = $state(false);
	let configSaving = $state(false);
	let configError = $state('');
	let storageProviders = $state<StorageProvider[]>([]);

	// Config form fields (mirrors BackupConfig)
	let cfgEnabled = $state(false);
	let cfgSchedule = $state('');
	let cfgRetain = $state(7);
	let cfgPreDeployEnabled = $state(false);
	let cfgPreDeployRetain = $state(5);
	let cfgKeepLocal = $state(true);
	let cfgProviderId = $state<string>('');

	// ── Lifecycle ─────────────────────────────────────────────────────────────

	$effect(() => {
		setUiContext(snapshotsContext());
		return () => clearUiContext();
	});

	// ── Create snapshot ───────────────────────────────────────────────────────

	const create = useAction(() => createSnapshot(), {
		invalidate: true,
		success: (snap) => `Snapshot created: ${snap.name}`,
		onSuccess: (snap) => {
			if (!snapshots.some((s) => s.id === snap.id)) snapshots = [snap, ...snapshots];
		}
	});

	// ── Delete ────────────────────────────────────────────────────────────────

	function handleDelete(snap: Snapshot) {
		successMessage = '';
		confirmAction({
			title: 'Delete this snapshot?',
			description: `${triggerLabel(snap)} from ${new Date(snap.created_at).toLocaleString()}. The local copy and any remote copy are removed. You can't restore from it afterwards.`,
			confirmLabel: 'Delete snapshot',
			pendingLabel: 'Deleting…',
			onconfirm: async () => {
				await deleteSnapshot(snap.id);
				snapshots = snapshots.filter((s) => s.id !== snap.id);
				await invalidateAll();
			}
		});
	}

	// ── Full restore ──────────────────────────────────────────────────────────

	function handleRestore(snap: Snapshot) {
		successMessage = '';
		confirmAction({
			title: 'Restore the whole server from this snapshot?',
			description: `Snapshot from ${new Date(snap.created_at).toLocaleString()}. Tond takes a pre-restore snapshot first, so you can undo this.`,
			details: [
				'Every app and database container is stopped.',
				"Tond's own database is replaced: apps, settings, users and sessions go back to this point. Everyone is signed out.",
				'All app volumes and database data are overwritten with the snapshot copy. Changes made since then are lost.',
				'Tond restarts when the restore finishes; the dashboard is unavailable for a minute.'
			],
			confirmLabel: 'Restore server',
			pendingLabel: 'Starting restore…',
			typedConfirmation: 'restore',
			onconfirm: async () => {
				const result = await restoreSnapshot(snap.id);
				successMessage = result.message;
			}
		});
	}

	// ── App rollback ──────────────────────────────────────────────────────────

	function handleRollback(snap: Snapshot) {
		const appName = snap.trigger_app ?? '';
		successMessage = '';
		confirmAction({
			title: `Roll back ${appName}'s data to this snapshot?`,
			description: `Snapshot taken before a deploy on ${new Date(snap.created_at).toLocaleString()}.`,
			details: [
				`${appName} is stopped while its volumes are replaced.`,
				`Everything written to ${appName}'s volumes since this snapshot is lost.`,
				'The app starts again afterwards. Its code and settings are not changed.'
			],
			confirmLabel: 'Roll back data',
			pendingLabel: 'Rolling back…',
			typedConfirmation: appName,
			onconfirm: async () => {
				const result = await rollbackAppToSnapshot(snap.id, appName);
				successMessage = result.message;
			}
		});
	}

	// ── Config panel ──────────────────────────────────────────────────────────

	async function openConfig() {
		showConfig = true;
		configLoading = true;
		configError = '';
		try {
			const [cfg, providers] = await Promise.all([getBackupConfig(), listStorageProviders()]);
			storageProviders = providers;
			cfgEnabled = cfg.enabled;
			cfgSchedule = cfg.full_schedule_cron;
			cfgRetain = cfg.full_retention_count;
			cfgPreDeployEnabled = cfg.pre_deploy_enabled;
			cfgPreDeployRetain = cfg.pre_deploy_retention_per_app;
			cfgKeepLocal = cfg.keep_local;
			cfgProviderId = cfg.storage_provider_id ?? '';
		} catch (e: any) {
			configError = e.message ?? 'Failed to load config';
		} finally {
			configLoading = false;
		}
	}

	async function handleSaveConfig() {
		configSaving = true;
		configError = '';
		try {
			await updateBackupConfig({
				enabled: cfgEnabled,
				full_schedule_cron: cfgSchedule,
				full_retention_count: cfgRetain,
				pre_deploy_enabled: cfgPreDeployEnabled,
				pre_deploy_retention_per_app: cfgPreDeployRetain,
				keep_local: cfgKeepLocal,
				storage_provider_id: cfgProviderId || null
			});
			showConfig = false;
			successMessage = 'Backup configuration saved.';
		} catch (e: any) {
			configError = e.message ?? 'Failed to save config';
		} finally {
			configSaving = false;
		}
	}

	// ── Helpers ───────────────────────────────────────────────────────────────

	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	function formatTime(iso: string): string {
		return new Date(iso).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	}

	function dayLabel(iso: string): string {
		const d = new Date(iso);
		const now = Date.now();
		const todayStr = new Date(now).toDateString();
		const yd = new Date(now - 86_400_000);
		const yesterdayStr = yd.toDateString();
		if (d.toDateString() === todayStr) return 'Today';
		if (d.toDateString() === yesterdayStr) return 'Yesterday';
		return d.toLocaleDateString(undefined, { weekday: 'long', month: 'short', day: 'numeric' });
	}

	/** Group snapshots by calendar date (newest first within each group) */
	let grouped = $derived.by(() => {
		const entries: Array<[string, Snapshot[]]> = [];
		for (const snap of snapshots) {
			const key = new Date(snap.created_at).toDateString();
			const existing = entries.find(([k]) => k === key);
			if (existing) {
				existing[1].push(snap);
			} else {
				entries.push([key, [snap]]);
			}
		}
		// Return array sorted newest-date first
		return entries.sort((a, b) => new Date(b[0]).getTime() - new Date(a[0]).getTime());
	});

	function triggerLabel(snap: Snapshot): string {
		const what = snap.scope.startsWith('database:')
			? `Database ${snap.scope.slice('database:'.length)}`
			: 'Full';
		if (snap.trigger === 'scheduled') return 'Scheduled · Full';
		if (snap.trigger === 'pre_deploy') return `Pre-deploy: ${snap.trigger_app ?? snap.scope}`;
		if (snap.trigger === 'manual') return `Manual · ${what}`;
		if (snap.trigger === 'pre_restore') return `Pre-restore · ${what}`;
		return snap.trigger;
	}

	/** Remote copy state of a snapshot, or null when it is local only. */
	function remoteLabel(snap: Snapshot): { text: string; failed: boolean } | null {
		switch (snap.remote_status) {
			case 'uploaded':
				return { text: 'Encrypted copy in remote storage', failed: false };
			case 'uploading':
				return { text: 'Uploading encrypted copy…', failed: false };
			case 'failed':
				return { text: 'Remote upload failed', failed: true };
			default:
				return null;
		}
	}

	function isFullScope(snap: Snapshot): boolean {
		return snap.scope === 'full';
	}

	function isPreDeploy(snap: Snapshot): boolean {
		return snap.trigger === 'pre_deploy';
	}
</script>

<div class="space-y-6">
	<PageHeader
		title="Snapshots"
		description="Point-in-time copies of Tond's database, app volumes and database data."
	>
		{#snippet actions()}
			{#if can('snapshots:create')}
				<Button variant="outline" size="sm" onclick={openConfig}>
					<Settings2 class="h-3.5 w-3.5" />
					Backup settings
				</Button>
				<Button
					size="sm"
					disabled={create.pending}
					aria-busy={create.pending}
					onclick={() => create.run()}
				>
					<Plus class="h-3.5 w-3.5" />
					{create.pending ? 'Creating…' : 'Create snapshot'}
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<InlineError message={error} ondismiss={() => (error = '')} />
	<InlineError title="Snapshot not created" message={create.error} ondismiss={create.clear} />
	<SuccessAlert
		message={successMessage || create.success}
		ondismiss={() => {
			successMessage = '';
			create.clear();
		}}
	/>

	<!-- Config panel (inline sheet) -->
	{#if showConfig}
		<div class="space-y-4 border bg-muted/30 p-5">
			<div class="flex items-center justify-between">
				<h2 class="text-sm font-semibold">Backup settings</h2>
				<Button variant="ghost" size="sm" class="h-7 text-xs" onclick={() => (showConfig = false)}>
					Close
				</Button>
			</div>

			{#if configLoading}
				<p class="text-xs text-muted-foreground">Loading…</p>
			{:else}
				<InlineError message={configError} />
				<div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
					<div class="flex items-center gap-2">
						<input type="checkbox" id="cfg-enabled" bind:checked={cfgEnabled} />
						<Label class="text-xs" for="cfg-enabled">Enable scheduled backups</Label>
					</div>
					<div class="grid gap-1.5">
						<Label class="text-xs" for="cfg-schedule">Schedule (cron)</Label>
						<Input
							id="cfg-schedule"
							class="h-8 text-xs"
							bind:value={cfgSchedule}
							placeholder="0 3 * * *"
						/>
					</div>
					<div class="grid gap-1.5">
						<Label class="text-xs" for="cfg-retain">Retain (full snapshots)</Label>
						<Input
							id="cfg-retain"
							type="number"
							class="h-8 text-xs"
							bind:value={cfgRetain}
							min={1}
						/>
					</div>
					<div class="flex items-center gap-2">
						<input type="checkbox" id="cfg-pre-deploy" bind:checked={cfgPreDeployEnabled} />
						<Label class="text-xs" for="cfg-pre-deploy">Enable pre-deploy snapshots</Label>
					</div>
					<div class="grid gap-1.5">
						<Label class="text-xs" for="cfg-pre-retain">Pre-deploy retain (per app)</Label>
						<Input
							id="cfg-pre-retain"
							type="number"
							class="h-8 text-xs"
							bind:value={cfgPreDeployRetain}
							min={1}
						/>
					</div>
					<div class="flex items-center gap-2">
						<input type="checkbox" id="cfg-local" bind:checked={cfgKeepLocal} />
						<Label class="text-xs" for="cfg-local">Keep local copy</Label>
					</div>
					<div class="grid gap-1.5 sm:col-span-2">
						<Label class="text-xs" for="cfg-provider">Remote storage provider</Label>
						<select
							id="cfg-provider"
							bind:value={cfgProviderId}
							class="h-8 w-full border bg-background px-2 text-xs"
						>
							<option value="">None — local only</option>
							{#each storageProviders as p (p.id)}
								<option value={p.id}>{p.name} ({p.kind.toUpperCase()})</option>
							{/each}
						</select>
						<p class="text-[10px] text-muted-foreground">
							<a href="/integrations" class="underline">Manage providers →</a>
						</p>
					</div>
				</div>
				<div class="flex gap-2 pt-2">
					<Button
						size="sm"
						class="h-8 text-xs uppercase"
						disabled={configSaving}
						onclick={handleSaveConfig}
					>
						{configSaving ? 'Saving…' : 'Save settings'}
					</Button>
					<Button
						variant="outline"
						size="sm"
						class="h-8 text-xs uppercase"
						onclick={() => (showConfig = false)}
					>
						Cancel
					</Button>
				</div>
			{/if}
		</div>
	{/if}

	<!-- Remote copies can only be restored with this server's secret key -->
	<div
		class="flex items-start gap-3 rounded border border-border bg-muted/30 px-4 py-3 text-xs text-muted-foreground"
		data-testid="snapshot-key-warning"
	>
		<KeyIcon class="mt-0.5 size-4 shrink-0 text-foreground" />
		<p>
			<span class="font-semibold text-foreground">Back up your secret key.</span>
			Copies in remote storage, and the secrets inside every snapshot, are encrypted with this server's
			<code class="font-mono">TOND_SECRET_KEY</code> (in
			<code class="font-mono">/opt/tond/data/tond.env</code>). Keep a copy of it somewhere safe off
			this server: restoring on a new server needs the same key.
		</p>
	</div>

	<!-- Timeline -->
	{#if snapshots.length === 0}
		<div class="border border-dashed py-16 text-center text-xs text-muted-foreground">
			No snapshots yet. Create one manually or enable scheduled backups in Config.
		</div>
	{:else}
		<div class="space-y-8">
			{#each grouped as [dateKey, daySnaps] (dateKey)}
				<div class="space-y-0">
					<!-- Date label -->
					<p class="mb-3 text-xs font-semibold tracking-widest text-muted-foreground uppercase">
						{dayLabel(daySnaps[0].created_at)}
					</p>

					<!-- Vertical timeline -->
					<div class="relative ml-4 space-y-0 border-l border-border">
						{#each daySnaps as snap (snap.id)}
							<div class="relative pb-1">
								<!-- Timeline dot -->
								{#if snap.status === 'failed'}
									<span
										class="absolute top-3 -left-1.75 flex h-3.5 w-3.5 items-center justify-center rounded-sm bg-destructive text-[8px] font-bold text-white"
										title="Failed">✕</span
									>
								{:else if isPreDeploy(snap)}
									<span
										class="absolute top-3 -left-1.75 flex h-3.5 w-3.5 rotate-45 items-center justify-center bg-warn"
										title="Pre-deploy"
									></span>
								{:else}
									<span
										class="absolute top-3 -left-1.75 h-3.5 w-3.5 rounded-full bg-info"
										title="Full snapshot"
									></span>
								{/if}

								<!-- Entry content -->
								<div class="ml-5 border bg-background text-xs">
									<div class="flex items-center justify-between px-4 py-3">
										<div class="flex min-w-0 flex-col gap-0.5">
											<div class="flex flex-wrap items-center gap-2">
												<span class="font-medium">{formatTime(snap.created_at)}</span>
												<span class="text-muted-foreground">{triggerLabel(snap)}</span>
												{#if snap.delta_bytes != null}
													<Badge variant="outline" class="h-4 px-1 text-[10px]">
														+{formatBytes(snap.delta_bytes)}
													</Badge>
												{/if}
												{#if snap.status !== 'completed'}
													<StatusBadge status={snap.status} kind="snapshot" size="sm" />
												{/if}
											</div>
											{#if remoteLabel(snap)}
												{@const remote = remoteLabel(snap)!}
												<span
													class="flex items-center gap-1 {remote.failed
														? 'text-destructive'
														: 'text-muted-foreground'}"
													title={snap.remote_error ?? ''}
												>
													<CloudIcon class="size-3" />
													{remote.text}{#if remote.failed && snap.remote_error}: {snap.remote_error}{/if}
												</span>
											{/if}
											{#if snap.deployment_id}
												<span class="text-muted-foreground">
													before deploy #{snap.deployment_id}
												</span>
											{/if}
											{#if snap.status === 'failed' && snap.error_message}
												<span class="text-destructive">{snap.error_message}</span>
											{/if}
										</div>

										<div class="ml-4 flex shrink-0 items-center gap-1">
											<!-- Rollback (pre-deploy) or Restore (full) -->
											{#if !can('snapshots:restore')}
												<!-- restore, roll back and delete need snapshots:restore -->
											{:else if isPreDeploy(snap) && snap.trigger_app}
												<Button
													variant="outline"
													size="sm"
													class="h-7 text-xs"
													onclick={() => handleRollback(snap)}
												>
													<RefreshCw class="h-3 w-3" />
													Roll back data
												</Button>
											{:else if isFullScope(snap) && snap.status === 'completed'}
												<Button
													variant="outline"
													size="sm"
													class="h-7 text-xs"
													onclick={() => handleRestore(snap)}
												>
													<RefreshCw class="h-3 w-3" />
													Restore…
												</Button>
											{/if}

											<!-- Delete -->
											{#if can('snapshots:restore')}
												<Button
													variant="ghost"
													size="icon"
													class="h-7 w-7 text-destructive hover:text-destructive"
													aria-label={`Delete snapshot ${snap.name}`}
													onclick={() => handleDelete(snap)}
												>
													<Trash2 class="h-3.5 w-3.5" />
												</Button>
											{/if}
										</div>
									</div>
								</div>
							</div>
						{/each}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>
