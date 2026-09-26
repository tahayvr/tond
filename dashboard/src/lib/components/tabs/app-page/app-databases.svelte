<script lang="ts">
	// Databases an app can reach: those in its project (shared network) and
	// those linked to it (the database joins the app's network).
	import {
		linkDatabaseApp,
		listAppDatabases,
		listDatabases,
		listProjects,
		unlinkDatabaseApp,
		type AppDatabaseAccess,
		type ManagedDatabase
	} from '$lib/api';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { untrack } from 'svelte';
	import LinkIcon from '~icons/ri/link';
	import LinkUnlinkIcon from '~icons/ri/link-unlink';
	import FolderIcon from '~icons/ri/folder-3-line';
	import DatabaseIcon from '~icons/ri/database-2-line';
	import RefreshIcon from '~icons/ri/refresh-line';

	interface Props {
		appId: string;
		appName: string;
		sourceType: string;
		/** The app's project, if any (its databases are reachable without a link) */
		projectId?: string | null;
	}

	let { appId, appName, sourceType, projectId = null }: Props = $props();

	let projectName = $state<string | null>(null);

	let reachable = $state<AppDatabaseAccess[]>([]);
	let databases = $state<ManagedDatabase[]>([]);
	let loading = $state(true);
	let loaded = $state(false);
	let loadError = $state('');
	let actionError = $state('');
	let notice = $state('');

	let selectedDbId = $state('');
	let linking = $state(false);

	let unlinkTarget = $state<AppDatabaseAccess | null>(null);
	let unlinkOpen = $state(false);
	let unlinking = $state(false);

	let isCompose = $derived(sourceType === 'compose');
	let candidates = $derived(
		databases
			.filter((d) => !reachable.some((r) => r.id === d.id))
			.sort((a, b) => a.name.localeCompare(b.name))
	);
	let selectedDb = $derived(databases.find((d) => d.id === selectedDbId) ?? null);
	let exampleHost = $derived(
		reachable[0] ? `${reachable[0].internal_host}:${reachable[0].internal_port}` : 'my-db:5432'
	);

	async function refreshReachable() {
		reachable = await listAppDatabases(appId);
	}

	async function load() {
		loading = true;
		loadError = '';
		try {
			const [nextReachable, nextDatabases, projects] = await Promise.all([
				listAppDatabases(appId),
				listDatabases(),
				projectId ? listProjects() : Promise.resolve([])
			]);
			reachable = nextReachable;
			databases = nextDatabases;
			projectName = projects.find((p) => p.id === projectId)?.name ?? null;
			loaded = true;
		} catch (e: any) {
			loadError = e.message ?? 'Could not load the databases this app can reach';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void appId;
		void projectId;
		// Compose apps can't reach managed databases: nothing to load.
		if (isCompose) return;
		untrack(load);
	});

	async function handleLink() {
		if (!selectedDb) return;
		linking = true;
		actionError = '';
		notice = '';
		const db = selectedDb;
		try {
			await linkDatabaseApp(db.id, appId);
			await refreshReachable();
			const entry = reachable.find((r) => r.id === db.id);
			notice = `${appName} can now reach ${db.name}${entry ? ` at ${entry.internal_host}:${entry.internal_port}` : ''}. No redeploy needed.`;
			selectedDbId = '';
		} catch (e: any) {
			actionError = e.message ?? `Could not link ${db.name}`;
		} finally {
			linking = false;
		}
	}

	function openUnlink(db: AppDatabaseAccess) {
		unlinkTarget = db;
		actionError = '';
		unlinkOpen = true;
	}

	async function handleConfirmUnlink() {
		if (!unlinkTarget) return;
		unlinking = true;
		actionError = '';
		try {
			await unlinkDatabaseApp(unlinkTarget.id, appId);
			await refreshReachable();
			notice = `${appName} can no longer reach ${unlinkTarget.name}.`;
			unlinkOpen = false;
		} catch (e: any) {
			actionError = e.message ?? `Could not unlink ${unlinkTarget.name}`;
		} finally {
			unlinking = false;
		}
	}
</script>

<div class="space-y-4 pt-4">
	<p class="max-w-2xl text-sm text-muted-foreground">
		{#if isCompose}
			Compose apps run on their own compose network, so they can't be linked to managed databases.
			Add the database as a service in the compose file instead.
		{:else}
			{appName} runs on its own network{projectName
				? `, shared with the other apps in ${projectName}`
				: ''}. It can reach
			{projectName ? `databases in ${projectName} and ` : ''}the databases you link here — by name,
			like <span class="font-mono text-xs text-foreground">{exampleHost}</span>. Nothing else on
			this server can reach it.
		{/if}
	</p>

	{#if !isCompose}
		{#if loading && !loaded}
			<div class="space-y-2" aria-busy="true" aria-label="Loading databases">
				<Skeleton class="h-10 w-full" />
				<Skeleton class="h-10 w-full" />
			</div>
		{:else if loadError && !loaded}
			<div
				class="space-y-2 rounded bg-destructive/10 px-3 py-2 text-sm text-destructive"
				role="alert"
			>
				<p>{loadError}</p>
				<Button variant="outline" size="sm" onclick={load}>
					<RefreshIcon class="size-3.5" /> Retry
				</Button>
			</div>
		{:else}
			{#if notice}
				<p class="rounded bg-success/10 px-3 py-2 text-xs text-success" role="status">{notice}</p>
			{/if}
			{#if actionError}
				<div
					class="flex items-start justify-between gap-3 rounded bg-destructive/10 px-3 py-2 text-xs text-destructive"
					role="alert"
				>
					<p>{actionError}</p>
					<button
						type="button"
						class="shrink-0 underline-offset-2 hover:underline"
						onclick={() => (actionError = '')}
					>
						Dismiss
					</button>
				</div>
			{/if}

			{#if reachable.length === 0}
				<div class="rounded border border-dashed border-border px-3 py-6 text-center">
					<DatabaseIcon class="mx-auto mb-2 size-5 text-muted-foreground/60" />
					<p class="text-sm text-muted-foreground">{appName} can't reach any database.</p>
					<p class="mt-1 text-xs text-muted-foreground/80">
						{databases.length === 0
							? 'Create a database first, then link it here.'
							: 'Link one below to give the app access.'}
					</p>
				</div>
			{:else}
				<ul class="divide-y divide-border rounded border border-border">
					{#each reachable as db (db.id)}
						<li class="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
							<div class="flex min-w-0 flex-wrap items-center gap-2">
								<a href="/databases/{db.id}" class="font-mono text-sm hover:underline">{db.name}</a>
								<span class="font-mono text-xs text-muted-foreground uppercase">{db.db_type}</span>
								{#if db.via === 'project'}
									<Badge variant="outline" class="gap-1"
										><FolderIcon /> via {projectName ?? 'project'}</Badge
									>
								{:else}
									<Badge variant="secondary" class="gap-1"><LinkIcon /> linked</Badge>
								{/if}
							</div>
							<div class="flex items-center gap-2">
								<span class="font-mono text-xs text-muted-foreground"
									>{db.internal_host}:{db.internal_port}</span
								>
								{#if db.via === 'link'}
									<Button
										variant="ghost"
										size="sm"
										class="h-7 text-muted-foreground hover:text-destructive"
										aria-label={`Unlink ${db.name}`}
										onclick={() => openUnlink(db)}
									>
										<LinkUnlinkIcon class="size-3.5" /> Unlink
									</Button>
								{/if}
							</div>
						</li>
					{/each}
				</ul>
			{/if}

			<div class="flex flex-col gap-2 sm:flex-row sm:items-center">
				<Select.Root type="single" bind:value={selectedDbId} disabled={candidates.length === 0}>
					<Select.Trigger class="w-full sm:w-72" aria-label="Database to link">
						{selectedDb?.name ??
							(candidates.length === 0 ? 'No other databases' : 'Choose a database to link')}
					</Select.Trigger>
					<Select.Content>
						{#each candidates as db (db.id)}
							<Select.Item value={db.id}>{db.name} ({db.db_type})</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<Button size="sm" onclick={handleLink} disabled={!selectedDb || linking}>
					<LinkIcon class="size-3.5" />
					{linking ? 'Linking…' : 'Link database'}
				</Button>
			</div>
		{/if}
	{/if}
</div>

<Dialog.Root bind:open={unlinkOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Unlink {unlinkTarget?.name}?</Dialog.Title>
			<Dialog.Description>
				{appName} loses its connection to {unlinkTarget?.name} right away. Open connections are cut, and
				requests that need the database will fail until you link it again.
			</Dialog.Description>
		</Dialog.Header>
		{#if actionError}
			<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
				{actionError}
			</p>
		{/if}
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (unlinkOpen = false)} disabled={unlinking}>
				Cancel
			</Button>
			<Button variant="destructive" onclick={handleConfirmUnlink} disabled={unlinking}>
				{unlinking ? 'Unlinking…' : 'Unlink'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
