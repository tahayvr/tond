<script lang="ts" module>
	import type { Snapshot } from '$lib/api';

	/** Human label for where a backup lives. */
	export function backupLocation(s: Snapshot): string {
		switch (s.remote_status) {
			case 'uploaded':
				return 'On this server and in remote storage';
			case 'uploading':
				return 'Uploading to remote storage…';
			case 'failed':
				return 'Remote upload failed; kept on this server';
			default:
				return s.local_path ? 'On this server' : 'In remote storage';
		}
	}

	export function formatBytes(bytes: number | null): string {
		if (bytes === null) return '—';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
		return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
	}
</script>

<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	// Backups of one managed database: back up now, list the backups that hold
	// it (its own and full-server snapshots), restore one with a typed
	// confirmation. A restore first backs up the current data.
	import { createDatabaseBackup, listDatabaseBackups, restoreDatabase } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import ArchiveIcon from '~icons/ri/archive-line';
	import HistoryIcon from '~icons/ri/history-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import LockIcon from '~icons/ri/lock-line';

	interface Props {
		dbId: string;
		dbName: string;
		/** snapshots:restore */
		canRestore: boolean;
		/** snapshots:create */
		canBackup?: boolean;
	}

	let { dbId, dbName, canRestore, canBackup = true }: Props = $props();

	let backups = $state<Snapshot[]>([]);
	let loading = $state(true);
	let loadError = $state('');
	let backingUp = $state(false);
	let backupError = $state('');
	let message = $state('');

	let restoreTarget = $state<Snapshot | null>(null);
	let restoreOpen = $state(false);
	let typed = $state('');
	let restoring = $state(false);
	let restoreError = $state('');
	let confirmed = $derived(typed.trim() === dbName);

	let completed = $derived(backups.filter((b) => b.status === 'completed'));

	async function load() {
		loading = true;
		loadError = '';
		try {
			backups = await listDatabaseBackups(dbId);
		} catch (e: any) {
			loadError = e.message ?? 'Could not load backups';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void dbId;
		void load();
	});

	async function handleBackup() {
		backingUp = true;
		backupError = '';
		message = '';
		try {
			const snap = await createDatabaseBackup(dbId);
			message =
				snap.remote_status === 'uploading'
					? `Backed up ${dbName}. An encrypted copy is uploading to remote storage.`
					: `Backed up ${dbName}.`;
			await load();
		} catch (e: any) {
			backupError = e.message ?? 'The backup failed';
		} finally {
			backingUp = false;
		}
	}

	function openRestore(snap: Snapshot) {
		restoreTarget = snap;
		typed = '';
		restoreError = '';
		restoreOpen = true;
	}

	async function handleRestore() {
		if (!restoreTarget || !confirmed) return;
		restoring = true;
		restoreError = '';
		try {
			const res = await restoreDatabase(restoreTarget.id, dbName);
			restoreOpen = false;
			message = res.message;
			await load();
		} catch (e: any) {
			restoreError = e.message ?? 'The restore failed. Nothing was changed.';
		} finally {
			restoring = false;
		}
	}

	function when(iso: string): string {
		return new Date(iso).toLocaleString(undefined, {
			dateStyle: 'medium',
			timeStyle: 'short'
		});
	}

	function kindLabel(s: Snapshot): string {
		if (s.scope === 'full') return 'Full server snapshot';
		if (s.trigger === 'pre_restore') return 'Before a restore';
		return 'Database backup';
	}
</script>

<div class="space-y-4" data-testid="database-backups">
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div class="space-y-1">
			<h2 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">Backups</h2>
			<p class="max-w-prose text-sm text-muted-foreground">
				A backup holds a dump of {dbName} and a copy of its data directory. Restoring one replaces the
				data in {dbName}; the current data is backed up first, so a restore can be undone.
			</p>
		</div>
		<Button size="sm" disabled={backingUp || !canBackup} onclick={handleBackup}>
			<ArchiveIcon class="size-4" />
			{backingUp ? 'Backing up…' : 'Back up now'}
		</Button>
	</div>

	{#if backupError}
		<div
			class="flex items-start justify-between gap-3 rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive"
		>
			<span>{backupError}</span>
			<button type="button" class="text-xs underline" onclick={() => (backupError = '')}>
				Dismiss
			</button>
		</div>
	{/if}
	{#if message}
		<div
			class="flex items-start justify-between gap-3 rounded-md bg-success/10 px-3 py-2 text-sm text-success"
			role="status"
		>
			<span>{message}</span>
			<button type="button" class="text-xs underline" onclick={() => (message = '')}>Dismiss</button
			>
		</div>
	{/if}

	{#if loading}
		<p class="text-xs text-muted-foreground">Loading backups…</p>
	{:else if loadError}
		<div class="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">
			{loadError}
			<button type="button" class="ml-2 text-xs underline" onclick={load}>Try again</button>
		</div>
	{:else if completed.length === 0}
		<div class="rounded border border-dashed border-border px-4 py-8 text-center">
			<p class="text-sm text-muted-foreground">
				No backups of {dbName} yet. Back it up now, or turn on scheduled snapshots in Snapshots.
			</p>
		</div>
	{:else}
		<ul class="divide-y divide-border overflow-hidden rounded border border-border">
			{#each completed as snap (snap.id)}
				<li class="flex flex-col gap-2 px-3 py-3 sm:flex-row sm:items-center sm:justify-between">
					<div class="min-w-0 space-y-0.5">
						<div class="flex flex-wrap items-center gap-2">
							<span class="text-sm font-medium">{when(snap.created_at)}</span>
							<Badge variant="outline" class="text-[10px]">{kindLabel(snap)}</Badge>
							{#if snap.remote_status === 'failed'}
								<Badge variant="destructive" class="text-[10px]">Upload failed</Badge>
							{/if}
						</div>
						<p class="truncate text-xs text-muted-foreground" title={snap.remote_error ?? ''}>
							{backupLocation(snap)}
							{#if snap.scope !== 'full'}
								· {formatBytes(snap.total_bytes)}
							{/if}
						</p>
					</div>
					{#if canRestore}
						<Button
							variant="outline"
							size="sm"
							class="shrink-0 self-start sm:self-auto"
							onclick={() => openRestore(snap)}
						>
							<HistoryIcon class="size-4" />
							Restore…
						</Button>
					{/if}
				</li>
			{/each}
		</ul>
		{#if !canRestore}
			<p class="flex items-center gap-1.5 text-xs text-muted-foreground">
				<LockIcon class="size-3.5" /> Your role can't restore databases.
			</p>
		{/if}
	{/if}

	<p class="flex items-start gap-1.5 text-xs text-muted-foreground">
		<KeyIcon class="mt-0.5 size-3.5 shrink-0" />
		<span>
			Copies in remote storage are encrypted with this server's secret key (<code class="font-mono"
				>TOND_SECRET_KEY</code
			>). To restore them on another server, that server needs the same key.
		</span>
	</p>
</div>

<Dialog.Root bind:open={restoreOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Restore {dbName}?</Dialog.Title>
			<Dialog.Description>
				{#if restoreTarget}
					All data in <strong>{dbName}</strong> is replaced with the copy from
					<strong>{when(restoreTarget.created_at)}</strong>. Anything written since then is lost
					from the database. The database restarts, and the current data is backed up first so you
					can undo this.
				{/if}
			</Dialog.Description>
		</Dialog.Header>

		<div class="grid gap-2 py-2">
			<Label for="restore-confirm" class="block leading-normal normal-case">
				Type <span class="font-mono font-semibold">{dbName}</span> to confirm
			</Label>
			<Input
				id="restore-confirm"
				bind:value={typed}
				autocomplete="off"
				spellcheck={false}
				disabled={restoring}
			/>
			<InlineError message={restoreError} />
		</div>

		<Dialog.Footer>
			<Button variant="outline" disabled={restoring} onclick={() => (restoreOpen = false)}>
				Cancel
			</Button>
			<Button variant="destructive" disabled={!confirmed || restoring} onclick={handleRestore}>
				{restoring ? 'Restoring…' : `Restore ${dbName}`}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
