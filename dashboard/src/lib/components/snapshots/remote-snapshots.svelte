<!--
  RemoteSnapshots — "Restore from storage": the snapshots found in a storage
  provider's bucket (from their manifests), so a fresh server can restore
  what another server uploaded. Restoring imports the snapshot into this
  server's list, then runs the chosen restore (whole server, one app, one
  database in place, or one database into a new database).
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		ApiError,
		ERROR_CODES,
		getBackupConfig,
		importRemoteSnapshot,
		listApps,
		listDatabases,
		listRemoteSnapshots,
		listStorageProviders,
		restoreDatabase,
		restoreDatabaseToNew,
		restoreSnapshot,
		rollbackAppToSnapshot,
		type RemoteSnapshot,
		type StorageProvider
	} from '$lib/api';
	import { errorMessage } from '$lib/action.svelte';
	import { confirmAction, type ConfirmOptions } from '$lib/confirm.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Badge from '$lib/components/ui/badge/badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import RefreshIcon from '~icons/ri/refresh-line';
	import HistoryIcon from '~icons/ri/history-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import CloudIcon from '~icons/ri/cloud-line';

	let providers = $state<StorageProvider[]>([]);
	let providerId = $state('');
	let serverApps = $state<string[]>([]);
	let serverDatabases = $state<string[]>([]);
	let setupLoaded = $state(false);
	let setupError = $state('');

	let remote = $state<RemoteSnapshot[]>([]);
	let listing = $state(false);
	let listError = $state('');
	let success = $state('');

	/** The snapshot whose restore options are open, and the choices in it. */
	let openId = $state<string | null>(null);
	let choice = $state('');
	let newDbName = $state('');

	async function loadSetup() {
		setupError = '';
		try {
			const [list, cfg, apps, dbs] = await Promise.all([
				listStorageProviders(),
				getBackupConfig(),
				listApps().catch(() => []),
				listDatabases().catch(() => [])
			]);
			providers = list;
			serverApps = apps.map((a) => a.name);
			serverDatabases = dbs.map((d) => d.name);
			providerId =
				cfg.storage_provider_id && list.some((p) => p.id === cfg.storage_provider_id)
					? cfg.storage_provider_id
					: (list[0]?.id ?? '');
		} catch (e) {
			setupError = `Couldn't load the storage providers: ${errorMessage(e)}`;
		} finally {
			setupLoaded = true;
		}
	}

	async function loadRemote(id: string) {
		listing = true;
		listError = '';
		try {
			remote = await listRemoteSnapshots(id);
		} catch (e) {
			remote = [];
			listError = `Couldn't read the bucket: ${errorMessage(e)} Check the provider's credentials with Test on the Integrations page.`;
		} finally {
			listing = false;
		}
	}

	$effect(() => {
		untrack(loadSetup);
	});

	// List the bucket of the chosen provider.
	$effect(() => {
		const id = providerId;
		if (!id) return;
		openId = null;
		untrack(() => loadRemote(id));
	});

	// ── Helpers ───────────────────────────────────────────────────────────────

	function formatBytes(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
		return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
	}

	function when(iso: string): string {
		return new Date(iso).toLocaleString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function scopeLabel(snap: RemoteSnapshot): string {
		if (!snap.scope) return 'Unknown contents';
		if (snap.scope === 'full') return 'Full';
		if (snap.scope.startsWith('database:')) return `Database ${snap.scope.slice(9)}`;
		return snap.scope;
	}

	interface RestoreOption {
		value: string;
		label: string;
		hint: string;
	}

	/** What a snapshot can restore on this server. */
	function optionsFor(snap: RemoteSnapshot): RestoreOption[] {
		const out: RestoreOption[] = [];
		if (snap.scope === 'full') {
			out.push({
				value: 'full',
				label: 'Everything',
				hint: "Tond's database, settings, every app and database. Tond restarts."
			});
			for (const app of snap.apps.filter((a) => serverApps.includes(a))) {
				out.push({ value: `app:${app}`, label: `App ${app}`, hint: 'Its volumes only.' });
			}
		}
		for (const db of snap.databases) {
			out.push(
				serverDatabases.includes(db)
					? { value: `db:${db}`, label: `Database ${db}`, hint: 'Replaces its data in place.' }
					: {
							value: `newdb:${db}`,
							label: `Database ${db}`,
							hint: 'Not on this server: restored into a new database.'
						}
			);
		}
		return out;
	}

	function missingApps(snap: RemoteSnapshot): string[] {
		return snap.scope === 'full' ? snap.apps.filter((a) => !serverApps.includes(a)) : [];
	}

	function handleOpen(snap: RemoteSnapshot) {
		success = '';
		if (openId === snap.id) {
			openId = null;
			return;
		}
		openId = snap.id;
		choice = optionsFor(snap)[0]?.value ?? '';
		const db = choice.startsWith('newdb:') ? choice.slice(6) : (snap.databases[0] ?? 'db');
		newDbName = `${db}-restored`;
	}

	/** Import the snapshot into this server's list; key mismatches say what to do. */
	async function importFirst(snap: RemoteSnapshot): Promise<string> {
		try {
			const row = await importRemoteSnapshot(snap.id, snap.provider_id);
			remote = remote.map((r) => (r.id === snap.id ? { ...r, imported: true } : r));
			return row.id;
		} catch (e) {
			if (e instanceof ApiError && e.code === ERROR_CODES.snapshotKeyMismatch) {
				throw new Error([e.message, e.hint].filter(Boolean).join(' '), { cause: e });
			}
			throw e;
		}
	}

	function confirmOptions(snap: RemoteSnapshot, value: string): ConfirmOptions {
		const at = when(snap.created_at);
		const download =
			'Tond downloads the encrypted archive and checks it before anything is replaced.';
		if (value === 'full') {
			return {
				title: 'Restore the whole server from this snapshot?',
				description: `${snap.name ?? 'Snapshot'} from ${at}, read from remote storage. Tond takes a pre-restore snapshot of this server first.`,
				details: [
					download,
					'Every app and database container on this server is stopped.',
					"Tond's own database is replaced: apps, settings, users and sessions become the snapshot's. Everyone is signed out.",
					'All app volumes and database data are overwritten with the snapshot copy.',
					'Tond restarts, then starts every app and database again, redeploying those whose containers are missing.'
				],
				confirmLabel: 'Restore server',
				pendingLabel: 'Starting restore…',
				typedConfirmation: 'restore',
				onconfirm: async () => {
					const id = await importFirst(snap);
					await restoreSnapshot(id);
					success =
						'Full restore started. Tond restarts when it finishes, then starts or redeploys every app and database. Sign in again in a few minutes (with an account from the restored server).';
				}
			};
		}
		if (value.startsWith('app:')) {
			const app = value.slice(4);
			return {
				title: `Roll back ${app}'s data to this snapshot?`,
				description: `Snapshot from ${at}, read from remote storage.`,
				details: [
					download,
					`${app} is stopped while its volumes are replaced.`,
					`Everything written to ${app}'s volumes since this snapshot is lost.`,
					'The app starts again afterwards. Its code and settings are not changed.'
				],
				confirmLabel: 'Roll back data',
				pendingLabel: 'Restoring…',
				typedConfirmation: app,
				onconfirm: async () => {
					const id = await importFirst(snap);
					const result = await rollbackAppToSnapshot(id, app);
					success = result.message;
				}
			};
		}
		if (value.startsWith('db:')) {
			const db = value.slice(3);
			return {
				title: `Restore ${db} from this snapshot?`,
				description: `Snapshot from ${at}, read from remote storage.`,
				details: [
					download,
					`Tond backs up ${db}'s current data first; nothing is restored if that backup fails.`,
					`${db}'s data is replaced with the snapshot's. Writes since then are lost.`,
					'The database container stops briefly.'
				],
				confirmLabel: 'Restore database',
				pendingLabel: 'Restoring…',
				typedConfirmation: db,
				onconfirm: async () => {
					const id = await importFirst(snap);
					const result = await restoreDatabase(id, db);
					success = result.message;
				}
			};
		}
		const source = value.slice(6);
		const name = newDbName.trim();
		return {
			title: `Restore ${source} into a new database?`,
			description: `Snapshot from ${at}, read from remote storage.`,
			details: [
				download,
				`Creates the database ${name} with ${source}'s engine and version, new credentials and its own port.`,
				'Nothing on this server is overwritten.'
			],
			confirmLabel: 'Create database',
			pendingLabel: 'Restoring…',
			onconfirm: async () => {
				const id = await importFirst(snap);
				const result = await restoreDatabaseToNew(id, { source, name });
				success = result.message;
				serverDatabases = [...serverDatabases, name];
			}
		};
	}

	async function handleReview(snap: RemoteSnapshot) {
		if (!choice) return;
		const done = await confirmAction(confirmOptions(snap, choice));
		if (done) openId = null;
	}

	let selectedProvider = $derived(providers.find((p) => p.id === providerId));
</script>

<div class="space-y-4">
	<p class="text-sm text-muted-foreground">
		Snapshots uploaded to remote storage, by this server or another one. Restore one to move a
		server or recover after losing one. The archives are encrypted: this server needs the same
		<code class="font-mono text-xs">TOND_SECRET_KEY</code> as the server that made them.
	</p>

	<InlineError message={setupError} ondismiss={() => (setupError = '')} />
	<SuccessAlert message={success} ondismiss={() => (success = '')} />

	{#if setupLoaded && providers.length === 0 && !setupError}
		<div class="space-y-2 border border-dashed px-4 py-10 text-center text-sm">
			<CloudIcon class="mx-auto size-5 text-muted-foreground" />
			<p class="font-medium">No storage provider yet</p>
			<p class="text-muted-foreground">
				Add the S3-compatible bucket your snapshots were uploaded to, with the same credentials,
				then come back here.
			</p>
			<Button href="/integrations" variant="outline" size="sm">Add a storage provider</Button>
		</div>
	{:else if providers.length > 0}
		<div class="flex flex-col gap-2 sm:flex-row sm:items-end">
			<div class="grid gap-1.5 sm:w-md">
				<Label class="text-xs" for="remote-provider">Storage provider</Label>
				<select
					id="remote-provider"
					bind:value={providerId}
					class="h-9 w-full border bg-background px-2 text-sm"
				>
					{#each providers as p (p.id)}
						<option value={p.id}>{p.name} ({p.config.bucket})</option>
					{/each}
				</select>
			</div>
			<Button
				variant="outline"
				size="sm"
				class="self-start sm:self-auto"
				disabled={listing || !providerId}
				aria-busy={listing}
				onclick={() => loadRemote(providerId)}
			>
				<RefreshIcon class="size-3.5" />
				{listing ? 'Reading bucket…' : 'Refresh'}
			</Button>
		</div>

		<InlineError title="Couldn't list remote snapshots" message={listError} />

		{#if listing && remote.length === 0}
			<p class="text-sm text-muted-foreground">Reading the bucket…</p>
		{:else if !listError && remote.length === 0}
			<div class="border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
				No Tond snapshots in {selectedProvider?.config.bucket ?? 'this bucket'}. Snapshots appear
				here once a server with this provider selected in Backup settings uploads one.
			</div>
		{:else}
			<ul class="space-y-2" aria-label="Remote snapshots">
				{#each remote as snap (snap.id)}
					{@const options = optionsFor(snap)}
					{@const missing = missingApps(snap)}
					<li class="border bg-background text-sm" data-testid="remote-snapshot">
						<div
							class="flex flex-col gap-3 px-4 py-3 sm:flex-row sm:items-center sm:justify-between"
						>
							<div class="min-w-0 space-y-1">
								<div class="flex flex-wrap items-center gap-2">
									<span class="font-medium">{snap.name ?? when(snap.created_at)}</span>
									<Badge variant="outline">{scopeLabel(snap)}</Badge>
									{#if snap.imported}
										<Badge variant="secondary">Already in list</Badge>
									{/if}
								</div>
								<p class="text-xs text-muted-foreground">
									{when(snap.created_at)} · {formatBytes(snap.archive_bytes)} · Tond {snap.tond_version}
								</p>
								{#if !snap.readable}
									<p class="flex items-start gap-1.5 text-xs text-warn">
										<KeyIcon class="mt-0.5 size-3.5 shrink-0" />
										<span>
											Made with a different key. This server's
											<code class="font-mono">TOND_SECRET_KEY</code> can't decrypt it. To restore
											it, set <code class="font-mono">TOND_SECRET_KEY</code> in
											<code class="font-mono">/opt/tond/data/tond.env</code> to the key of the server
											that made it and restart tond-server.
										</span>
									</p>
								{/if}
							</div>
							{#if snap.readable && options.length > 0}
								<Button
									variant="outline"
									size="sm"
									class="shrink-0 self-start sm:self-auto"
									aria-expanded={openId === snap.id}
									aria-controls={`restore-${snap.id}`}
									onclick={() => handleOpen(snap)}
								>
									<HistoryIcon class="size-3.5" />
									Restore…
								</Button>
							{/if}
						</div>

						{#if openId === snap.id}
							<div id={`restore-${snap.id}`} class="space-y-3 border-t bg-muted/30 px-4 py-3">
								<fieldset class="space-y-2">
									<legend class="mb-1 text-xs font-semibold">What to restore</legend>
									{#each options as opt (opt.value)}
										<label class="flex cursor-pointer items-start gap-2">
											<input
												type="radio"
												name={`restore-choice-${snap.id}`}
												value={opt.value}
												bind:group={choice}
												class="mt-1"
											/>
											<span>
												<span class="font-medium">{opt.label}</span>
												<span class="block text-xs text-muted-foreground">{opt.hint}</span>
											</span>
										</label>
									{/each}
								</fieldset>
								{#if missing.length > 0}
									<p class="text-xs text-muted-foreground">
										Not on this server: {missing.join(', ')}. Restore everything to bring these apps
										back.
									</p>
								{/if}
								{#if choice.startsWith('newdb:')}
									<div class="grid gap-1.5 sm:w-80">
										<Label class="text-xs" for={`new-db-${snap.id}`}>New database name</Label>
										<Input id={`new-db-${snap.id}`} class="h-9" bind:value={newDbName} />
									</div>
								{/if}
								<div class="flex flex-wrap gap-2">
									<Button
										size="sm"
										variant={choice.startsWith('newdb:') ? 'default' : 'destructive'}
										disabled={!choice || (choice.startsWith('newdb:') && !newDbName.trim())}
										onclick={() => handleReview(snap)}
									>
										Review restore…
									</Button>
									<Button size="sm" variant="ghost" onclick={() => (openId = null)}>Cancel</Button>
								</div>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	{/if}
</div>
