<script lang="ts">
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
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import { setUiContext, clearUiContext, snapshotsContext } from '$lib/ui-context.svelte';
	import Trash2 from '~icons/ri/delete-bin-line';
	import RefreshCw from '~icons/ri/refresh-line';
	import Download from '~icons/ri/download-line';
	import Settings2 from '~icons/ri/equalizer-line';
	import Plus from '~icons/ri/add-line';
	import ChevronDown from '~icons/ri/arrow-down-s-line';
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
	let creating = $state(false);
	let createError = $state('');
	let successMessage = $state('');

	// Restore / rollback
	let confirmRestoreId = $state<string | null>(null);
	let confirmRollbackId = $state<string | null>(null);
	let rollbackAppName = $state('');
	let restoringId = $state<string | null>(null);

	// Delete
	let deletingId = $state<string | null>(null);

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

	async function handleCreate() {
		creating = true;
		createError = '';
		successMessage = '';
		try {
			const snap = await createSnapshot();
			snapshots = [snap, ...snapshots];
			successMessage = `Snapshot created: ${snap.name}`;
		} catch (e: any) {
			createError = e.message ?? 'Failed to create snapshot';
		} finally {
			creating = false;
		}
	}

	// ── Delete ────────────────────────────────────────────────────────────────

	async function handleDelete(id: string) {
		deletingId = id;
		error = '';
		try {
			await deleteSnapshot(id);
			snapshots = snapshots.filter((s) => s.id !== id);
		} catch (e: any) {
			error = e.message ?? 'Failed to delete snapshot';
		} finally {
			deletingId = null;
		}
	}

	// ── Full restore ──────────────────────────────────────────────────────────

	async function handleRestore(id: string) {
		restoringId = id;
		confirmRestoreId = null;
		error = '';
		successMessage = '';
		try {
			const result = await restoreSnapshot(id);
			successMessage = result.message;
		} catch (e: any) {
			error = e.message ?? 'Restore failed';
		} finally {
			restoringId = null;
		}
	}

	// ── App rollback ──────────────────────────────────────────────────────────

	async function handleRollback(snapshotId: string) {
		if (!rollbackAppName.trim()) return;
		restoringId = snapshotId;
		confirmRollbackId = null;
		error = '';
		successMessage = '';
		try {
			const result = await rollbackAppToSnapshot(snapshotId, rollbackAppName.trim());
			successMessage = result.message;
		} catch (e: any) {
			error = e.message ?? 'Rollback failed';
		} finally {
			restoringId = null;
			rollbackAppName = '';
		}
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
	<!-- Header -->
	<div class="flex h-14 items-center justify-between">
		<h2 class="text-3xl font-extrabold uppercase">Snapshots</h2>
		<div class="flex items-center gap-2">
			<Button variant="outline" size="sm" class="h-8 text-xs uppercase" onclick={openConfig}>
				<Settings2 class="h-3.5 w-3.5" />
				Config
			</Button>
			<Button size="sm" class="h-8 text-xs uppercase" disabled={creating} onclick={handleCreate}>
				<Plus class="h-3.5 w-3.5" />
				{creating ? 'Creating…' : 'Create Snapshot'}
			</Button>
		</div>
	</div>

	<!-- Feedback banners -->
	{#if error}
		<ErrorAlert title="Snapshot Error" description={error} />
	{/if}

	{#if createError}
		<div class="rounded border border-destructive bg-destructive/10 p-3 text-xs text-destructive">
			{createError}
		</div>
	{/if}

	{#if successMessage}
		<div class="rounded border border-green-500 bg-green-500/10 p-3 text-xs text-green-600">
			{successMessage}
		</div>
	{/if}

	<!-- Config panel (inline sheet) -->
	{#if showConfig}
		<div class="space-y-4 border bg-muted/30 p-5">
			<div class="flex items-center justify-between">
				<p class="text-xs font-semibold uppercase">Backup Configuration</p>
				<Button variant="ghost" size="sm" class="h-7 text-xs" onclick={() => (showConfig = false)}>
					Close
				</Button>
			</div>

			{#if configLoading}
				<p class="text-xs text-muted-foreground">Loading…</p>
			{:else}
				{#if configError}
					<p class="text-xs text-destructive">{configError}</p>
				{/if}
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
						{configSaving ? 'Saving…' : 'Save Config'}
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
										class="absolute top-3 -left-1.75 flex h-3.5 w-3.5 rotate-45 items-center justify-center bg-amber-500"
										title="Pre-deploy"
									></span>
								{:else}
									<span
										class="absolute top-3 -left-1.75 h-3.5 w-3.5 rounded-full bg-blue-500"
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
												{#if snap.status === 'failed'}
													<Badge variant="destructive" class="h-4 px-1 text-[10px]">Failed</Badge>
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
											<!-- Download -->
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7"
												aria-label="Download"
												onclick={() => window.open(`/api/snapshots/${snap.id}/download`)}
											>
												<Download class="h-3.5 w-3.5" />
											</Button>

											<!-- Rollback (pre-deploy) or Restore (full) -->
											{#if isPreDeploy(snap) && snap.trigger_app}
												{#if confirmRollbackId === snap.id}
													<div class="flex items-center gap-1">
														<Input
															class="h-7 w-32 text-xs"
															bind:value={rollbackAppName}
															placeholder={snap.trigger_app}
														/>
														<Button
															size="sm"
															class="h-7 text-xs uppercase"
															disabled={restoringId === snap.id}
															onclick={() => handleRollback(snap.id)}
														>
															{restoringId === snap.id ? '…' : 'Confirm'}
														</Button>
														<Button
															variant="outline"
															size="sm"
															class="h-7 text-xs uppercase"
															onclick={() => (confirmRollbackId = null)}
														>
															Cancel
														</Button>
													</div>
												{:else}
													<Button
														variant="outline"
														size="sm"
														class="h-7 text-xs uppercase"
														onclick={() => {
															confirmRollbackId = snap.id;
															rollbackAppName = snap.trigger_app ?? '';
														}}
													>
														<RefreshCw class="h-3 w-3" />
														Rollback
													</Button>
												{/if}
											{:else if isFullScope(snap) && snap.status === 'completed'}
												{#if confirmRestoreId === snap.id}
													<div class="flex items-center gap-1">
														<span class="text-[10px] text-destructive"
															>Full restore — are you sure?</span
														>
														<Button
															size="sm"
															class="h-7 text-xs uppercase"
															disabled={restoringId === snap.id}
															onclick={() => handleRestore(snap.id)}
														>
															{restoringId === snap.id ? 'Restoring…' : 'Yes, restore'}
														</Button>
														<Button
															variant="outline"
															size="sm"
															class="h-7 text-xs uppercase"
															onclick={() => (confirmRestoreId = null)}
														>
															Cancel
														</Button>
													</div>
												{:else}
													<Button
														variant="outline"
														size="sm"
														class="h-7 text-xs uppercase"
														onclick={() => (confirmRestoreId = snap.id)}
													>
														<RefreshCw class="h-3 w-3" />
														Restore
														<ChevronDown class="h-3 w-3" />
													</Button>
												{/if}
											{/if}

											<!-- Delete -->
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7 text-destructive hover:text-destructive"
												aria-label={`Delete snapshot ${snap.name}`}
												disabled={deletingId === snap.id}
												onclick={() => handleDelete(snap.id)}
											>
												<Trash2 class="h-3.5 w-3.5" />
											</Button>
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
