<script lang="ts">
	import ConnectionString from '$lib/components/databases/connection-string.svelte';
	import { page } from '$app/state';
	import {
		getDatabase,
		stopDatabase,
		startDatabase,
		deleteDatabase,
		updateDatabase,
		updateDatabaseCredentials,
		redeployDatabase,
		listProjects,
		type ManagedDatabaseDetail,
		type Project
	} from '$lib/api';
	import { goto } from '$app/navigation';
	import {
		setUiContext,
		clearUiContext,
		databaseContext,
		openFeedPanel
	} from '$lib/ui-context.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import MetricsPanel from '$lib/components/metrics/metrics-panel.svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { untrack } from 'svelte';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';
	import Copy from '~icons/ri/file-copy-line';
	import Check from '~icons/ri/check-line';
	import Pencil from '~icons/ri/pencil-line';
	import GlobeIcon from '~icons/ri/global-line';
	import LockIcon from '~icons/ri/lock-line';
	import AlertIcon from '~icons/ri/alert-line';
	import { isAdmin } from '$lib/auth.svelte';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();
	type DatabaseDetailData = {
		db: ManagedDatabaseDetail;
		projects: Project[];
	};

	let db = $state<ManagedDatabaseDetail | null>(null);
	let activeTab = $state('overview');
	let error = $state('');
	let projects = $state<Project[]>([]);
	let showDeleteDialog = $state(false);
	let acting = $state(false);
	let redeploying = $state(false);

	// URL editing state
	let editingExternal = $state(false);
	let editingInternal = $state(false);
	let externalDraft = $state('');
	let internalDraft = $state('');
	let savingExternal = $state(false);
	let savingInternal = $state(false);
	let urlError = $state('');

	// Image editing state
	let editingImage = $state(false);
	let imageDraft = $state('');
	let savingImage = $state(false);
	let imageError = $state('');

	// Public access state
	let publicDialogOpen = $state(false);
	let savingPublic = $state(false);
	let publicError = $state('');

	// Password changes run inside the running database (server-side ALTER USER),
	// so only passwords are editable and only while the database is running.
	type CredField = 'db_name' | 'db_user' | 'db_password' | 'root_password';
	type PasswordField = 'db_password' | 'root_password';
	let editingCredField = $state<PasswordField | null>(null);
	let credDraft = $state('');
	let credDraftVisible = $state(false);
	let savingCred = $state(false);
	let credError = $state('');
	let credDialogOpen = $state(false);
	let credSuccess = $state('');
	let isRedisLike = $derived(db?.db_type === 'redis' || db?.db_type === 'keydb');
	let dbRunning = $derived(db?.status === 'running');

	function isPasswordField(field: CredField): field is PasswordField {
		return field === 'db_password' || field === 'root_password';
	}

	function credLabel(field: PasswordField | null): string {
		return field === 'root_password' ? 'Root password' : 'Password';
	}

	// Per-row reveal/copy state
	let revealedKeys = new SvelteSet<string>();
	let copiedEnvVar = $state<string | null>(null);
	let copyTimeout: ReturnType<typeof setTimeout> | null = null;

	// Limits editing state
	let memoryLimitDraft = $state<number | null>(null);
	let memoryUnitDraft = $state<'mb' | 'gb'>('mb');
	let savingLimits = $state(false);
	let limitsError = $state('');
	let limitsSaved = $state(false);
	let limitsSavedTimeout: ReturnType<typeof setTimeout> | null = null;

	let dbId = $derived(page.params.id || params.id);

	async function refreshDatabaseData() {
		error = '';
		try {
			[db, projects] = await Promise.all([getDatabase(dbId), listProjects()]);
			if (db) {
				setUiContext(databaseContext(db));
				initLimitsDraft();
			}
		} catch (e: any) {
			error = e.message ?? 'Failed to load database';
		}
	}

	async function setPublicAccess(enabled: boolean) {
		if (!db) return;
		savingPublic = true;
		publicError = '';
		try {
			db = await updateDatabase(dbId, { public_access: enabled });
			publicDialogOpen = false;
		} catch (e: any) {
			publicError = e.message ?? 'Failed to change public access';
		} finally {
			savingPublic = false;
		}
	}

	async function handleStop() {
		acting = true;
		error = '';
		openFeedPanel();
		try {
			await stopDatabase(dbId);
			await refreshDatabaseData();
		} catch (e: any) {
			error = e.message ?? 'Failed to stop database';
		} finally {
			acting = false;
		}
	}

	async function handleStart() {
		acting = true;
		error = '';
		openFeedPanel();
		try {
			await startDatabase(dbId);
			await refreshDatabaseData();
		} catch (e: any) {
			error = e.message ?? 'Failed to start database';
		} finally {
			acting = false;
		}
	}

	async function handleRedeploy() {
		redeploying = true;
		error = '';
		openFeedPanel();
		try {
			db = await redeployDatabase(dbId);
			if (db) setUiContext(databaseContext(db));
		} catch (e: any) {
			error = e.message ?? 'Failed to redeploy database';
		} finally {
			redeploying = false;
		}
	}

	async function handleDelete() {
		try {
			await deleteDatabase(dbId);
			goto('/databases');
		} catch (e: any) {
			error = e.message ?? 'Failed to delete database';
		}
	}

	// ── URL editing ──

	function startEditExternal() {
		externalDraft = db?.custom_external_url ?? db?.connection_string ?? '';
		editingExternal = true;
		urlError = '';
	}

	function cancelEditExternal() {
		editingExternal = false;
		urlError = '';
	}

	async function saveExternal() {
		savingExternal = true;
		urlError = '';
		try {
			db = await updateDatabase(dbId, { custom_external_url: externalDraft || null });
			editingExternal = false;
		} catch (e: any) {
			urlError = e.message ?? 'Failed to save external URL';
		} finally {
			savingExternal = false;
		}
	}

	async function clearExternal() {
		savingExternal = true;
		urlError = '';
		try {
			db = await updateDatabase(dbId, { custom_external_url: '' });
			editingExternal = false;
		} catch (e: any) {
			urlError = e.message ?? 'Failed to clear external URL override';
		} finally {
			savingExternal = false;
		}
	}

	function startEditInternal() {
		internalDraft = db?.custom_internal_url ?? db?.internal_connection_string ?? '';
		editingInternal = true;
		urlError = '';
	}

	function cancelEditInternal() {
		editingInternal = false;
		urlError = '';
	}

	async function saveInternal() {
		savingInternal = true;
		urlError = '';
		try {
			db = await updateDatabase(dbId, { custom_internal_url: internalDraft || null });
			editingInternal = false;
		} catch (e: any) {
			urlError = e.message ?? 'Failed to save internal URL';
		} finally {
			savingInternal = false;
		}
	}

	async function clearInternal() {
		savingInternal = true;
		urlError = '';
		try {
			db = await updateDatabase(dbId, { custom_internal_url: '' });
			editingInternal = false;
		} catch (e: any) {
			urlError = e.message ?? 'Failed to clear internal URL override';
		} finally {
			savingInternal = false;
		}
	}

	// ── Image editing ──

	function startEditImage() {
		imageDraft = db?.custom_image ?? db?.image_used ?? '';
		editingImage = true;
		imageError = '';
	}

	function cancelEditImage() {
		editingImage = false;
		imageError = '';
	}

	async function saveImage() {
		savingImage = true;
		imageError = '';
		try {
			db = await updateDatabase(dbId, { custom_image: imageDraft || null });
			editingImage = false;
		} catch (e: any) {
			imageError = e.message ?? 'Failed to save image override';
		} finally {
			savingImage = false;
		}
	}

	async function clearImage() {
		savingImage = true;
		imageError = '';
		try {
			db = await updateDatabase(dbId, { custom_image: '' });
			editingImage = false;
		} catch (e: any) {
			imageError = e.message ?? 'Failed to clear image override';
		} finally {
			savingImage = false;
		}
	}

	// ── Credentials editing ──

	function startEditCred(field: PasswordField) {
		credDraft = '';
		credDraftVisible = false;
		editingCredField = field;
		credError = '';
		credDialogOpen = true;
	}

	function cancelEditCred() {
		credDialogOpen = false;
		editingCredField = null;
		credError = '';
	}

	/** A 24-character random password (letters and digits, safe in connection URLs). */
	function handleGeneratePassword() {
		const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789';
		const bytes = crypto.getRandomValues(new Uint8Array(24));
		credDraft = Array.from(bytes, (b) => alphabet[b % alphabet.length]).join('');
		credDraftVisible = true;
	}

	async function saveCred() {
		if (!editingCredField) return;
		if (!credDraft.trim()) {
			credError = 'Enter a new password.';
			return;
		}
		savingCred = true;
		credError = '';
		credSuccess = '';
		try {
			const field = editingCredField;
			const res = await updateDatabaseCredentials(dbId, { [field]: credDraft });
			db = res.database;
			if (db) setUiContext(databaseContext(db));
			credSuccess = `${credLabel(field)} changed inside the database and saved. Update any app that still connects with the old password, then redeploy it.`;
			credDialogOpen = false;
			editingCredField = null;
			credDraft = '';
		} catch (e: any) {
			credError = e.message ?? 'Could not change the password';
		} finally {
			savingCred = false;
		}
	}

	function toggleReveal(envKey: string) {
		if (revealedKeys.has(envKey)) {
			revealedKeys.delete(envKey);
		} else {
			revealedKeys.add(envKey);
		}
	}

	async function copyEnvVar(text: string, key: string) {
		await navigator.clipboard.writeText(text);
		copiedEnvVar = key;
		if (copyTimeout) clearTimeout(copyTimeout);
		copyTimeout = setTimeout(() => (copiedEnvVar = null), 2000);
	}

	// ── Limits ──

	function initLimitsDraft() {
		if (!db) return;
		if (db.memory_limit === null) {
			memoryLimitDraft = null;
			memoryUnitDraft = 'mb';
		} else {
			const mb = db.memory_limit / (1024 * 1024);
			if (mb >= 1024 && mb % 1024 === 0) {
				memoryLimitDraft = mb / 1024;
				memoryUnitDraft = 'gb';
			} else {
				memoryLimitDraft = mb;
				memoryUnitDraft = 'mb';
			}
		}
	}

	async function saveLimits() {
		savingLimits = true;
		limitsError = '';
		try {
			let memoryLimitBytes: number | null = null;
			if (memoryLimitDraft !== null && memoryLimitDraft > 0) {
				memoryLimitBytes =
					memoryUnitDraft === 'gb'
						? memoryLimitDraft * 1024 * 1024 * 1024
						: memoryLimitDraft * 1024 * 1024;
			}
			db = await updateDatabase(dbId, { memory_limit: memoryLimitBytes });
			if (db) setUiContext(databaseContext(db));
			limitsSaved = true;
			if (limitsSavedTimeout) clearTimeout(limitsSavedTimeout);
			limitsSavedTimeout = setTimeout(() => (limitsSaved = false), 2000);
		} catch (e: any) {
			limitsError = e.message ?? 'Failed to save limits';
		} finally {
			savingLimits = false;
		}
	}

	// Compute the environment variable names and values for the current engine.
	let envVars = $derived.by(() => {
		if (!db) return [];
		const type = db.db_type;
		const vars: {
			envKey: string;
			credField: CredField;
			label: string;
			value: string | null;
		}[] = [];

		if (type === 'postgres') {
			if (db.db_name !== null)
				vars.push({
					envKey: 'POSTGRES_DB',
					credField: 'db_name',
					label: 'Database name',
					value: db.db_name
				});
			if (db.db_user !== null)
				vars.push({
					envKey: 'POSTGRES_USER',
					credField: 'db_user',
					label: 'Username',
					value: db.db_user
				});
			if (db.db_password !== null)
				vars.push({
					envKey: 'POSTGRES_PASSWORD',
					credField: 'db_password',
					label: 'Password',
					value: db.db_password
				});
		} else if (type === 'mysql' || type === 'mariadb') {
			const prefix = type === 'mariadb' ? 'MARIADB' : 'MYSQL';
			if (db.db_name !== null)
				vars.push({
					envKey: `${prefix}_DATABASE`,
					credField: 'db_name',
					label: 'Database name',
					value: db.db_name
				});
			if (db.db_user !== null)
				vars.push({
					envKey: `${prefix}_USER`,
					credField: 'db_user',
					label: 'Username',
					value: db.db_user
				});
			if (db.db_password !== null)
				vars.push({
					envKey: `${prefix}_PASSWORD`,
					credField: 'db_password',
					label: 'Password',
					value: db.db_password
				});
			if (db.root_password !== null)
				vars.push({
					envKey: `${prefix}_ROOT_PASSWORD`,
					credField: 'root_password',
					label: 'Root password',
					value: db.root_password
				});
		} else if (type === 'mongodb') {
			if (db.db_name !== null)
				vars.push({
					envKey: 'MONGO_INITDB_DATABASE',
					credField: 'db_name',
					label: 'Database name',
					value: db.db_name
				});
			if (db.db_user !== null)
				vars.push({
					envKey: 'MONGO_INITDB_ROOT_USERNAME',
					credField: 'db_user',
					label: 'Username',
					value: db.db_user
				});
			if (db.db_password !== null)
				vars.push({
					envKey: 'MONGO_INITDB_ROOT_PASSWORD',
					credField: 'db_password',
					label: 'Password',
					value: db.db_password
				});
		} else if (type === 'redis' || type === 'keydb') {
			if (db.db_user !== null)
				vars.push({
					envKey: 'REDIS_USERNAME',
					credField: 'db_user',
					label: 'Username',
					value: db.db_user
				});
			if (db.db_password !== null)
				vars.push({
					envKey: 'REDIS_PASSWORD',
					credField: 'db_password',
					label: 'Password',
					value: db.db_password
				});
		}

		return vars;
	});

	function statusVariant(
		status: string
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		switch (status) {
			case 'running':
				return 'default';
			case 'failed':
				return 'destructive';
			case 'stopped':
				return 'secondary';
			default:
				return 'outline';
		}
	}

	function formatMemory(bytes: number | null): string {
		if (bytes === null) return 'Unlimited';
		const mb = bytes / (1024 * 1024);
		if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
		return `${mb.toFixed(0)} MB`;
	}

	// Listen for the footer action button's stop/start event so the page
	// refreshes when triggered from the footer.
	// Re-seed local state when the route data changes. Only `data` is tracked:
	// reading `db` here after assigning it would re-run this effect forever.
	$effect(() => {
		const routeData = data as DatabaseDetailData;
		untrack(() => {
			db = routeData.db;
			projects = routeData.projects;
			if (db) {
				setUiContext(databaseContext(db));
				initLimitsDraft();
			}
		});
	});

	// Listen for the footer action button's stop/start event so the page
	// refreshes when triggered from the footer.
	$effect(() => {
		function handleDbStatusChanged() {
			void refreshDatabaseData();
		}
		window.addEventListener('tond:db-status-changed', handleDbStatusChanged);
		return () => window.removeEventListener('tond:db-status-changed', handleDbStatusChanged);
	});

	$effect(() => {
		return () => clearUiContext();
	});
</script>

{#if db}
	<div class="space-y-6">
		<!-- Breadcrumb -->
		<Breadcrumb items={[{ label: 'Databases', href: '/databases' }, { label: db.name }]} />

		<!-- Error banner -->
		{#if error}
			<ErrorAlert title="Error" description={error} />
		{/if}

		<!-- Header -->
		<div class="flex h-14 items-center justify-between">
			<div>
				<h2 class="text-3xl font-extrabold uppercase">{db.name}</h2>
				<div class="mt-2 flex items-center gap-3 text-sm text-muted-foreground">
					<Badge variant={statusVariant(db.status)}>{db.status}</Badge>
					<span class="font-mono text-xs uppercase">{db.db_type}</span>
					{#if db.host_port}
						<span class="font-mono text-xs">:{db.host_port}</span>
					{/if}
				</div>
			</div>
			<div class="flex gap-2">
				{#if db.status === 'running'}
					<Button variant="outline" size="sm" disabled={acting || redeploying} onclick={handleStop}>
						{acting ? 'Stopping...' : 'Stop'}
					</Button>
				{:else if db.status === 'stopped' || db.status === 'failed'}
					<Button
						variant="outline"
						size="sm"
						disabled={acting || redeploying}
						onclick={handleStart}
					>
						{acting ? 'Starting...' : 'Start'}
					</Button>
				{/if}
				<Button
					variant="outline"
					size="sm"
					disabled={acting || redeploying}
					onclick={handleRedeploy}
				>
					{redeploying ? 'Redeploying...' : 'Redeploy'}
				</Button>
				<Button variant="destructive" size="sm" onclick={() => (showDeleteDialog = true)}>
					Delete
				</Button>
			</div>
		</div>

		<Separator />

		<!-- Tabs -->
		<Tabs.Root bind:value={activeTab}>
			<Tabs.List class="max-w-full justify-start overflow-x-auto">
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="environment">Environment</Tabs.Trigger>
				<Tabs.Trigger value="limits">Limits</Tabs.Trigger>
				<Tabs.Trigger value="metrics">Metrics</Tabs.Trigger>
			</Tabs.List>

			<!-- ── Overview tab ── -->
			<Tabs.Content value="overview" class="mt-4 space-y-4">
				<!-- Info grid -->
				<div class="grid gap-4 sm:grid-cols-2">
					<!-- Connection strings -->
					<div class="space-y-4 rounded-lg border p-4">
						<h3 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
							Connection
						</h3>

						{#if urlError}
							<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive">{urlError}</p>
						{/if}

						<!-- External URL -->
						<div class="space-y-2">
							<div class="flex items-center justify-between">
								<p class="text-xs font-medium text-muted-foreground">
									{db.public_access ? 'External' : 'From this server (localhost)'}
								</p>
								{#if !editingExternal}
									<Button
										variant="ghost"
										size="sm"
										class="h-6 px-2 text-xs"
										onclick={startEditExternal}
									>
										Edit
									</Button>
								{/if}
							</div>

							{#if editingExternal}
								<div class="space-y-2">
									<Input
										class="font-mono text-xs"
										placeholder="e.g. postgres://user:pass@host:5432/db"
										bind:value={externalDraft}
										disabled={savingExternal}
									/>
									<div class="flex gap-2">
										<Button size="sm" disabled={savingExternal} onclick={saveExternal}>
											{savingExternal ? 'Saving...' : 'Save'}
										</Button>
										<Button
											variant="outline"
											size="sm"
											disabled={savingExternal}
											onclick={cancelEditExternal}
										>
											Cancel
										</Button>
										{#if db.custom_external_url}
											<Button
												variant="ghost"
												size="sm"
												class="ml-auto text-muted-foreground"
												disabled={savingExternal}
												onclick={clearExternal}
											>
												Reset to default
											</Button>
										{/if}
									</div>
								</div>
							{:else if db.connection_string}
								<div>
									<ConnectionString value={db.connection_string} />
									{#if db.custom_external_url}
										<span class="mt-1 block text-xs text-muted-foreground/60">custom override</span>
									{/if}
								</div>
							{:else}
								<p class="text-xs text-muted-foreground/60">Not available (database is stopped)</p>
							{/if}
						</div>

						<!-- Internal URL -->
						<div class="space-y-2">
							<div class="flex items-center justify-between">
								<p class="text-xs font-medium text-muted-foreground">
									Internal (within project network)
								</p>
								{#if !editingInternal}
									<Button
										variant="ghost"
										size="sm"
										class="h-6 px-2 text-xs"
										onclick={startEditInternal}
									>
										Edit
									</Button>
								{/if}
							</div>

							{#if editingInternal}
								<div class="space-y-2">
									<Input
										class="font-mono text-xs"
										placeholder="e.g. postgres://user:pass@db-alias:5432/db"
										bind:value={internalDraft}
										disabled={savingInternal}
									/>
									<div class="flex gap-2">
										<Button size="sm" disabled={savingInternal} onclick={saveInternal}>
											{savingInternal ? 'Saving...' : 'Save'}
										</Button>
										<Button
											variant="outline"
											size="sm"
											disabled={savingInternal}
											onclick={cancelEditInternal}
										>
											Cancel
										</Button>
										{#if db.custom_internal_url}
											<Button
												variant="ghost"
												size="sm"
												class="ml-auto text-muted-foreground"
												disabled={savingInternal}
												onclick={clearInternal}
											>
												Reset to default
											</Button>
										{/if}
									</div>
								</div>
							{:else if db.internal_connection_string}
								<div>
									<ConnectionString value={db.internal_connection_string} />
									{#if db.custom_internal_url}
										<span class="mt-1 block text-xs text-muted-foreground/60">custom override</span>
									{/if}
								</div>
							{:else}
								<p class="text-xs text-muted-foreground/60">
									{db.project_id ? 'Not available (database is stopped)' : 'No project assigned'}
								</p>
							{/if}
						</div>
					</div>

					<!-- Configuration -->
					<div class="space-y-3 rounded-lg border p-4">
						<h3 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
							Configuration
						</h3>

						<dl class="space-y-2 text-sm">
							<div class="flex justify-between">
								<dt class="text-muted-foreground">Engine</dt>
								<dd class="font-mono font-medium uppercase">{db.db_type}</dd>
							</div>

							<!-- Docker image row -->
							<div class="flex flex-col gap-1">
								<div class="flex items-center justify-between">
									<dt class="text-muted-foreground">Image</dt>
									{#if !editingImage && isAdmin()}
										<Button
											variant="ghost"
											size="sm"
											class="h-6 px-2 text-xs"
											onclick={startEditImage}
										>
											Edit
										</Button>
									{/if}
								</div>
								{#if editingImage}
									<div class="space-y-2">
										{#if imageError}
											<p class="rounded bg-destructive/10 px-2 py-1 text-xs text-destructive">
												{imageError}
											</p>
										{/if}
										<Input
											class="font-mono text-xs"
											placeholder="e.g. postgres:15-alpine"
											bind:value={imageDraft}
											disabled={savingImage}
										/>
										<div class="flex gap-2">
											<Button size="sm" disabled={savingImage} onclick={saveImage}>
												{savingImage ? 'Saving...' : 'Save'}
											</Button>
											<Button
												variant="outline"
												size="sm"
												disabled={savingImage}
												onclick={cancelEditImage}
											>
												Cancel
											</Button>
											{#if db.custom_image}
												<Button
													variant="ghost"
													size="sm"
													class="ml-auto text-muted-foreground"
													disabled={savingImage}
													onclick={clearImage}
												>
													Reset to default
												</Button>
											{/if}
										</div>
									</div>
								{:else}
									<dd class="flex items-center gap-2">
										<span class="font-mono text-xs">{db.custom_image ?? db.image_used ?? '—'}</span>
										{#if db.custom_image}
											<span class="text-xs text-muted-foreground/60">(pending redeploy)</span>
										{/if}
									</dd>
								{/if}
							</div>

							<div class="flex justify-between">
								<dt class="text-muted-foreground">Memory limit</dt>
								<dd class="font-mono font-medium">{formatMemory(db.memory_limit)}</dd>
							</div>
							{#if db.project_id}
								<div class="flex justify-between">
									<dt class="text-muted-foreground">Project</dt>
									<dd>
										<a href="/projects/{db.project_id}" class="text-sm hover:underline">
											{projects.find((p) => p.id === db!.project_id)?.name ?? db.project_id}
										</a>
									</dd>
								</div>
							{/if}
							<div class="flex justify-between">
								<dt class="text-muted-foreground">Created</dt>
								<dd class="font-mono text-xs">{new Date(db.created_at).toLocaleString()}</dd>
							</div>
						</dl>
					</div>
				</div>

				<!-- Network access (H6): private by default, explicit opt-in for public -->
				<div class="space-y-3 rounded-lg border p-4" data-testid="db-network-access">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<h3 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
							Network access
						</h3>
						{#if db.public_access}
							<Badge variant="destructive" class="gap-1"><GlobeIcon class="size-3" /> Public</Badge>
						{:else}
							<Badge variant="outline" class="gap-1"><LockIcon class="size-3" /> Private</Badge>
						{/if}
					</div>

					{#if publicError}
						<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive">
							{publicError}
						</p>
					{/if}

					{#if db.public_access}
						<p class="text-sm text-muted-foreground">
							Port <span class="font-mono">{db.host_port ?? '—'}</span> is open to the internet. Anyone
							who can reach this server can try to log in, so use a strong password and turn this off
							when you no longer need it.
						</p>
						{#if db.public_connection_string}
							<div class="space-y-1">
								<p class="text-xs font-medium text-muted-foreground">Public connection string</p>
								<ConnectionString value={db.public_connection_string} />
								{#if db.public_connection_string.includes('<server-ip>')}
									<p class="text-xs text-muted-foreground/70">
										Replace <span class="font-mono">&lt;server-ip&gt;</span> with this server's public
										IP, or set a dashboard domain in Settings.
									</p>
								{/if}
							</div>
						{/if}
					{:else}
						<p class="text-sm text-muted-foreground">
							Only reachable from this server on
							<span class="font-mono">127.0.0.1:{db.host_port ?? '—'}</span>{db.project_id
								? ' and from apps in its project'
								: ''}. To connect from your computer, use an SSH tunnel:
							<code class="rounded bg-muted px-1 font-mono text-xs"
								>ssh -L {db.host_port ?? 5432}:localhost:{db.host_port ?? 5432} root@your-server</code
							>
						</p>
					{/if}

					{#if isAdmin()}
						<div class="flex flex-wrap items-center gap-3">
							{#if db.public_access}
								<Button
									variant="outline"
									size="sm"
									disabled={savingPublic}
									onclick={() => setPublicAccess(false)}
								>
									<LockIcon class="size-4" />
									{savingPublic ? 'Updating…' : 'Make private'}
								</Button>
							{:else}
								<Button
									variant="outline"
									size="sm"
									disabled={savingPublic}
									onclick={() => (publicDialogOpen = true)}
								>
									<GlobeIcon class="size-4" />
									Enable public access…
								</Button>
							{/if}
							<p class="text-xs text-muted-foreground/70">
								Changing this restarts the database container (a few seconds of downtime). Data is
								kept.
							</p>
						</div>
					{:else}
						<p class="flex items-center gap-1.5 text-xs text-muted-foreground">
							<LockIcon class="size-3.5" /> Only admins can change network access.
						</p>
					{/if}
				</div>

				<Dialog.Root bind:open={publicDialogOpen}>
					<Dialog.Content class="max-w-md">
						<Dialog.Header>
							<Dialog.Title class="flex items-center gap-2">
								<AlertIcon class="size-5 text-destructive" /> Expose {db.name} to the internet?
							</Dialog.Title>
							<Dialog.Description>
								Port <span class="font-mono">{db.host_port ?? '—'}</span> will accept connections from
								any address. Docker opens it directly, so the server firewall (UFW) does not block it.
								Bots scan these ports constantly: only do this if you must connect from outside and the
								password is strong. An SSH tunnel is safer.
							</Dialog.Description>
						</Dialog.Header>
						{#if publicError}
							<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive">
								{publicError}
							</p>
						{/if}
						<Dialog.Footer>
							<Button
								variant="outline"
								disabled={savingPublic}
								onclick={() => (publicDialogOpen = false)}>Cancel</Button
							>
							<Button
								variant="destructive"
								disabled={savingPublic}
								onclick={() => setPublicAccess(true)}
							>
								{savingPublic ? 'Restarting database…' : 'Make public'}
							</Button>
						</Dialog.Footer>
					</Dialog.Content>
				</Dialog.Root>
			</Tabs.Content>

			<!-- ── Environment tab ── -->
			<Tabs.Content value="environment" class="mt-4 space-y-4">
				{#if envVars.length === 0}
					<div class="rounded border border-dashed border-border py-8 text-center">
						<p class="text-sm text-muted-foreground">
							No credentials are configured for this database engine.
						</p>
					</div>
				{:else}
					<!-- How credential changes work -->
					{#if credSuccess}
						<div
							class="flex items-start justify-between gap-3 rounded-lg bg-success/10 px-4 py-3 text-sm text-success"
							role="status"
						>
							<p>{credSuccess}</p>
							<button
								type="button"
								class="text-xs underline-offset-2 hover:underline"
								onclick={() => (credSuccess = '')}
							>
								Dismiss
							</button>
						</div>
					{/if}
					<div
						class="rounded-lg border border-border bg-muted/30 px-4 py-3 text-xs text-muted-foreground"
					>
						{#if isRedisLike}
							Redis and KeyDB passwords can't be changed in place yet.
						{:else}
							<p>
								<span class="font-semibold text-foreground">Changing a password</span> updates it inside
								the running database right away, then saves it here. Apps that use the old password must
								be updated and redeployed.
							</p>
							<p class="mt-1">
								The database name and username can't be changed after creation.
								{#if !dbRunning}
									<span class="text-warn">Start the database to change a password.</span>
								{/if}
							</p>
						{/if}
					</div>

					<!-- Credentials table -->
					<div class="overflow-hidden rounded border border-border">
						<table class="w-full text-sm">
							<thead>
								<tr class="border-b border-border bg-muted/50">
									<th
										class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
										>Key</th
									>
									<th
										class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
										>Value</th
									>
									<th class="w-24 px-3 py-2"></th>
								</tr>
							</thead>
							<tbody>
								{#each envVars as v, i (v.envKey)}
									{@const revealed = revealedKeys.has(v.envKey)}
									<tr class={i < envVars.length - 1 ? 'border-b border-border' : ''}>
										<td class="px-3 py-2 font-mono text-xs font-medium">{v.envKey}</td>
										<td class="max-w-0 truncate px-3 py-2 font-mono text-xs text-muted-foreground">
											{#if !v.value}
												<span class="italic">empty</span>
											{:else if revealed}
												{v.value}
											{:else}
												<span class="tracking-widest">••••••••</span>
											{/if}
										</td>
										<td class="px-3 py-2">
											<div class="flex items-center justify-end gap-1">
												{#if v.value}
													<Button
														variant="ghost"
														size="icon"
														class="h-7 w-7 text-muted-foreground hover:text-foreground"
														aria-label={revealed ? 'Hide value' : 'Reveal value'}
														onclick={() => toggleReveal(v.envKey)}
													>
														{#if revealed}
															<EyeOff class="h-3.5 w-3.5" />
														{:else}
															<Eye class="h-3.5 w-3.5" />
														{/if}
													</Button>
													<Button
														variant="ghost"
														size="icon"
														class="h-7 w-7 text-muted-foreground hover:text-foreground"
														aria-label="Copy value"
														onclick={() => copyEnvVar(v.value ?? '', v.envKey)}
													>
														{#if copiedEnvVar === v.envKey}
															<Check class="h-3.5 w-3.5 text-green-500" />
														{:else}
															<Copy class="h-3.5 w-3.5" />
														{/if}
													</Button>
												{/if}
												{#if isPasswordField(v.credField) && !isRedisLike}
													{@const field = v.credField}
													<Button
														variant="ghost"
														size="icon"
														class="h-7 w-7 text-muted-foreground hover:text-foreground"
														aria-label={`Change ${credLabel(field).toLowerCase()}`}
														title={dbRunning
															? `Change ${credLabel(field).toLowerCase()}`
															: 'Start the database to change its password'}
														disabled={!dbRunning}
														onclick={() => startEditCred(field)}
													>
														<Pencil class="h-3.5 w-3.5" />
													</Button>
												{/if}
											</div>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>

					<!-- Env block copy-all -->
					<div class="space-y-2 rounded-lg border p-4">
						<div class="flex items-center justify-between">
							<p class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
								Copy all as env block
							</p>
							<Button
								variant="ghost"
								size="sm"
								class="h-6 px-2 text-xs"
								onclick={() =>
									copyEnvVar(
										envVars.map((v) => `${v.envKey}=${v.value ?? ''}`).join('\n'),
										'__all__'
									)}
							>
								{copiedEnvVar === '__all__' ? 'Copied!' : 'Copy'}
							</Button>
						</div>
						<!-- Values stay hidden here too unless revealed in the table above;
						     Copy always copies the real values. -->
						<pre
							class="overflow-x-auto rounded bg-muted px-3 py-2 font-mono text-xs leading-relaxed">{envVars
								.map(
									(v) => `${v.envKey}=${revealedKeys.has(v.envKey) ? (v.value ?? '') : '••••••••'}`
								)
								.join('\n')}</pre>
					</div>
				{/if}
			</Tabs.Content>

			<!-- ── Limits tab ── -->
			<Tabs.Content value="limits" class="mt-4 space-y-6 pt-2">
				<div class="max-w-sm space-y-6">
					<div class="space-y-2">
						<Label>Memory Limit</Label>
						<div class="flex gap-2">
							<Input
								type="number"
								bind:value={memoryLimitDraft}
								placeholder="Unlimited"
								min={1}
								disabled={savingLimits}
							/>
							<Select.Root type="single" bind:value={memoryUnitDraft}>
								<Select.Trigger class="w-24" disabled={savingLimits}>
									{memoryUnitDraft.toUpperCase()}
								</Select.Trigger>
								<Select.Content>
									<Select.Item value="mb">MB</Select.Item>
									<Select.Item value="gb">GB</Select.Item>
								</Select.Content>
							</Select.Root>
						</div>
						<p class="text-xs text-muted-foreground">
							Leave empty for unlimited memory. Recommended: 256 MB for PostgreSQL/MySQL, 128 MB for
							Redis.
						</p>
					</div>

					{#if limitsError}
						<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive">
							{limitsError}
						</p>
					{/if}

					<div class="flex items-center gap-3">
						<Button onclick={saveLimits} disabled={savingLimits}>
							{savingLimits ? 'Saving...' : 'Save'}
						</Button>
						{#if limitsSaved}
							<span class="flex items-center gap-1 text-xs text-green-600 dark:text-green-400">
								<Check class="h-3.5 w-3.5" /> Saved
							</span>
						{/if}
					</div>
				</div>
			</Tabs.Content>

			<!-- ── Metrics tab (mounted only while open, so it polls only then) ── -->
			<Tabs.Content value="metrics">
				{#if activeTab === 'metrics'}
					{#key db.id}
						<MetricsPanel owner="databases" id={db.id} noun="database" />
					{/key}
				{/if}
			</Tabs.Content>
		</Tabs.Root>
	</div>
{:else}
	<div class="py-12 text-center text-muted-foreground">Database not found</div>
{/if}

<!-- Change password dialog -->
<Dialog.Root bind:open={credDialogOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Change {credLabel(editingCredField).toLowerCase()}</Dialog.Title>
			<Dialog.Description>
				Changes the password inside the running database; apps using the old password must be
				updated.
			</Dialog.Description>
		</Dialog.Header>
		<form
			class="space-y-4 py-2"
			onsubmit={(e) => {
				e.preventDefault();
				saveCred();
			}}
		>
			<div class="space-y-2">
				<div class="flex items-center justify-between">
					<Label for="cred-value">New {credLabel(editingCredField).toLowerCase()}</Label>
					<button
						type="button"
						class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
						onclick={handleGeneratePassword}
						disabled={savingCred}
					>
						Generate
					</button>
				</div>
				<div class="flex gap-2">
					<Input
						id="cred-value"
						bind:value={credDraft}
						class="font-mono"
						type={credDraftVisible ? 'text' : 'password'}
						autocomplete="new-password"
						spellcheck={false}
						disabled={savingCred}
						autofocus
					/>
					<Button
						type="button"
						variant="ghost"
						size="icon"
						aria-label={credDraftVisible ? 'Hide password' : 'Show password'}
						onclick={() => (credDraftVisible = !credDraftVisible)}
					>
						{#if credDraftVisible}
							<EyeOff class="h-4 w-4" />
						{:else}
							<Eye class="h-4 w-4" />
						{/if}
					</Button>
				</div>
			</div>
			{#if credError}
				<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
					{credError}
				</p>
			{/if}
			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={cancelEditCred} disabled={savingCred}>
					Cancel
				</Button>
				<Button type="submit" disabled={savingCred || !credDraft.trim()}>
					{savingCred ? 'Changing…' : 'Change password'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>

<!-- Delete confirmation dialog -->
<ConfirmDeleteDialog
	bind:open={showDeleteDialog}
	title={`Delete "${db?.name}"?`}
	description="This will stop the container and permanently remove all data. This action cannot be undone."
	confirmation={db?.name ?? ''}
	onconfirm={handleDelete}
/>
