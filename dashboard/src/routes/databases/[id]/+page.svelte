<script lang="ts">
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
	import { SvelteSet } from 'svelte/reactivity';
	import Eye from '@lucide/svelte/icons/eye';
	import EyeOff from '@lucide/svelte/icons/eye-off';
	import Copy from '@lucide/svelte/icons/copy';
	import Check from '@lucide/svelte/icons/check';
	import Pencil from '@lucide/svelte/icons/pencil';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();
	type DatabaseDetailData = {
		db: ManagedDatabaseDetail;
		projects: Project[];
	};

	let db = $state<ManagedDatabaseDetail | null>(null);
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

	// Credentials editing state
	let editingCredField = $state<'db_name' | 'db_user' | 'db_password' | 'root_password' | null>(
		null
	);
	let credDraft = $state('');
	let savingCred = $state(false);
	let credError = $state('');
	let credDialogOpen = $state(false);
	let credRedeployNeeded = $state(false);

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
			credRedeployNeeded = false;
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

	function startEditCred(field: 'db_name' | 'db_user' | 'db_password' | 'root_password') {
		credDraft =
			field === 'db_name'
				? (db?.db_name ?? '')
				: field === 'db_user'
					? (db?.db_user ?? '')
					: field === 'root_password'
						? (db?.root_password ?? '')
						: (db?.db_password ?? '');
		editingCredField = field;
		credError = '';
		credDialogOpen = true;
	}

	function cancelEditCred() {
		credDialogOpen = false;
		editingCredField = null;
		credError = '';
	}

	async function saveCred() {
		if (!editingCredField) return;
		savingCred = true;
		credError = '';
		try {
			const res = await updateDatabaseCredentials(dbId, { [editingCredField]: credDraft || null });
			db = res.database;
			credRedeployNeeded = res.requires_redeploy;
			credDialogOpen = false;
			editingCredField = null;
		} catch (e: any) {
			credError = e.message ?? 'Failed to save credential';
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
			credField: 'db_name' | 'db_user' | 'db_password' | 'root_password';
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
	$effect(() => {
		const routeData = data as DatabaseDetailData;
		db = routeData.db;
		projects = routeData.projects;
		if (db) {
			setUiContext(databaseContext(db));
			initLimitsDraft();
		}
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
		<Tabs.Root value="overview">
			<Tabs.List>
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="environment">Environment</Tabs.Trigger>
				<Tabs.Trigger value="limits">Limits</Tabs.Trigger>
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
								<p class="text-xs font-medium text-muted-foreground">External</p>
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
									<code
										class="block w-full truncate rounded bg-muted px-3 py-2 font-mono text-xs"
										title={db.connection_string}
									>
										{db.connection_string}
									</code>
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
									<code
										class="block w-full truncate rounded bg-muted px-3 py-2 font-mono text-xs"
										title={db.internal_connection_string}
									>
										{db.internal_connection_string}
									</code>
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
									{#if !editingImage}
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
					<!-- Pending-redeploy notice -->
					<div
						class={credRedeployNeeded
							? 'rounded-lg border border-amber-500/50 bg-amber-500/10 px-4 py-3 text-xs text-amber-700 dark:text-amber-300'
							: 'rounded-lg border border-amber-500/30 bg-amber-500/5 px-4 py-3 text-xs text-muted-foreground'}
					>
						{#if credRedeployNeeded}
							<span class="font-semibold text-amber-600 dark:text-amber-400"
								>Redeploy required:</span
							>
							Credential changes have been saved and will take effect after a
						{:else}
							<span class="font-semibold text-amber-600 dark:text-amber-400">Note:</span>
							Credential changes are saved immediately but only take effect in the container after a
						{/if}
						<button
							class="font-semibold underline underline-offset-2 hover:text-foreground"
							onclick={handleRedeploy}
							disabled={redeploying}
						>
							{redeploying ? 'redeploying...' : 'Redeploy'}
						</button>.
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
												<Button
													variant="ghost"
													size="icon"
													class="h-7 w-7 text-muted-foreground hover:text-foreground"
													aria-label="Edit credential"
													onclick={() => startEditCred(v.credField)}
												>
													<Pencil class="h-3.5 w-3.5" />
												</Button>
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
						<pre
							class="overflow-x-auto rounded bg-muted px-3 py-2 font-mono text-xs leading-relaxed">{envVars
								.map((v) => `${v.envKey}=${v.value ?? ''}`)
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
		</Tabs.Root>
	</div>
{:else}
	<div class="py-12 text-center text-muted-foreground">Database not found</div>
{/if}

<!-- Edit credential dialog -->
<Dialog.Root bind:open={credDialogOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Edit Credential</Dialog.Title>
			<Dialog.Description>
				Update the value for
				<span class="font-mono font-medium">
					{envVars.find((v) => v.credField === editingCredField)?.envKey ?? ''}
				</span>.
			</Dialog.Description>
		</Dialog.Header>
		<div class="space-y-4 py-2">
			<div class="space-y-2">
				<Label for="cred-value">Value</Label>
				<Input
					id="cred-value"
					bind:value={credDraft}
					class="font-mono"
					type={editingCredField === 'db_password' || editingCredField === 'root_password'
						? 'password'
						: 'text'}
					disabled={savingCred}
					autofocus
					onkeydown={(e) => {
						if (e.key === 'Enter') saveCred();
					}}
				/>
			</div>
			{#if credError}
				<p class="text-xs text-destructive">{credError}</p>
			{/if}
		</div>
		<Dialog.Footer>
			<Button variant="outline" onclick={cancelEditCred} disabled={savingCred}>Cancel</Button>
			<Button onclick={saveCred} disabled={savingCred}>
				{savingCred ? 'Saving...' : 'Save'}
			</Button>
		</Dialog.Footer>
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
