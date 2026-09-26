<script lang="ts">
	// Which apps can reach a database. Apps in the database's project share its
	// network; any other app needs a link, which attaches the database to that
	// app's network (see networks.rs on the server).
	import {
		getDatabaseAccess,
		linkDatabaseApp,
		listApps,
		unlinkDatabaseApp,
		type App,
		type DatabaseAccess,
		type DatabaseAppRef
	} from '$lib/api';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { untrack } from 'svelte';
	import { can } from '$lib/auth.svelte';
	import LinkIcon from '~icons/ri/link';
	import LinkUnlinkIcon from '~icons/ri/link-unlink';
	import FolderIcon from '~icons/ri/folder-3-line';
	import RefreshIcon from '~icons/ri/refresh-line';

	interface Props {
		databaseId: string;
		databaseName: string;
	}

	let { databaseId, databaseName }: Props = $props();

	let access = $state<DatabaseAccess | null>(null);
	let apps = $state<App[]>([]);
	let loading = $state(true);
	let loadError = $state('');
	let actionError = $state('');
	let notice = $state('');

	let selectedAppId = $state('');
	let linking = $state(false);

	/** Linking changes network reach on both sides: db:write + apps:write. */
	let canLink = $derived(can('db:write') && can('apps:write'));

	let unlinkTarget = $state<DatabaseAppRef | null>(null);
	let unlinkOpen = $state(false);
	let unlinking = $state(false);

	let endpoint = $derived(access ? `${access.internal_host}:${access.internal_port}` : '');

	/** Apps that could be linked: not compose, not already reaching the database. */
	let candidates = $derived.by(() => {
		if (!access) return [];
		const reachable = new Set([
			...access.project_apps.map((a) => a.id),
			...access.linked_apps.map((a) => a.id)
		]);
		return apps
			.filter((a) => !reachable.has(a.id) && a.source_type !== 'compose')
			.sort((a, b) => a.name.localeCompare(b.name));
	});
	let selectedApp = $derived(apps.find((a) => a.id === selectedAppId) ?? null);
	let hasAccess = $derived(
		(access?.project_apps.length ?? 0) + (access?.linked_apps.length ?? 0) > 0
	);

	async function load() {
		loading = true;
		loadError = '';
		try {
			[access, apps] = await Promise.all([getDatabaseAccess(databaseId), listApps()]);
		} catch (e: any) {
			loadError = e.message ?? 'Could not load which apps can reach this database';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void databaseId;
		untrack(load);
	});

	async function handleLink() {
		if (!selectedApp) return;
		linking = true;
		actionError = '';
		notice = '';
		try {
			access = await linkDatabaseApp(databaseId, selectedApp.id);
			notice = `${selectedApp.name} can now reach ${databaseName} at ${endpoint}. No redeploy needed.`;
			selectedAppId = '';
		} catch (e: any) {
			actionError = e.message ?? `Could not link ${selectedApp.name}`;
		} finally {
			linking = false;
		}
	}

	function openUnlink(app: DatabaseAppRef) {
		unlinkTarget = app;
		actionError = '';
		unlinkOpen = true;
	}

	async function handleConfirmUnlink() {
		if (!unlinkTarget) return;
		unlinking = true;
		actionError = '';
		try {
			access = await unlinkDatabaseApp(databaseId, unlinkTarget.id);
			notice = `${unlinkTarget.name} can no longer reach ${databaseName}.`;
			unlinkOpen = false;
		} catch (e: any) {
			actionError = e.message ?? `Could not unlink ${unlinkTarget.name}`;
		} finally {
			unlinking = false;
		}
	}
</script>

<div class="space-y-3 rounded-lg border p-4" data-testid="db-app-access">
	<div class="flex flex-wrap items-center justify-between gap-2">
		<h3 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
			Apps with access
		</h3>
		{#if access}
			<span class="font-mono text-xs text-muted-foreground">{endpoint}</span>
		{/if}
	</div>

	{#if loading && !access}
		<div class="space-y-2" aria-busy="true" aria-label="Loading apps with access">
			<Skeleton class="h-4 w-2/3" />
			<Skeleton class="h-10 w-full" />
		</div>
	{:else if loadError && !access}
		<div
			class="space-y-2 rounded bg-destructive/10 px-3 py-2 text-sm text-destructive"
			role="alert"
		>
			<p>{loadError}</p>
			<Button variant="outline" size="sm" onclick={load}>
				<RefreshIcon class="size-3.5" /> Retry
			</Button>
		</div>
	{:else if access}
		<p class="text-sm text-muted-foreground">
			Every app runs on its own network. Only the apps below can reach
			<span class="font-medium text-foreground">{databaseName}</span>, by the hostname
			<span class="font-mono text-xs text-foreground">{access.internal_host}</span>{access.project
				? `: apps in ${access.project.name} share its network, and other apps need a link`
				: ': link an app to give it access'}.
		</p>

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

		{#if hasAccess}
			<ul class="divide-y divide-border rounded border border-border">
				{#each access.project_apps as app (app.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
						<a href="/apps/{app.id}" class="font-mono text-sm hover:underline">{app.name}</a>
						<Badge variant="outline" class="gap-1">
							<FolderIcon /> via {access.project?.name ?? 'project'}
						</Badge>
					</li>
				{/each}
				{#each access.linked_apps as app (app.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
						<div class="flex min-w-0 items-center gap-2">
							<a href="/apps/{app.id}" class="truncate font-mono text-sm hover:underline"
								>{app.name}</a
							>
							<Badge variant="secondary" class="gap-1"><LinkIcon /> linked</Badge>
						</div>
						{#if canLink}
							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-muted-foreground hover:text-destructive"
								aria-label={`Unlink ${app.name}`}
								onclick={() => openUnlink(app)}
							>
								<LinkUnlinkIcon class="size-3.5" /> Unlink
							</Button>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<div class="rounded border border-dashed border-border px-3 py-5 text-center">
				<p class="text-sm text-muted-foreground">No app can reach {databaseName} yet.</p>
				<p class="mt-1 text-xs text-muted-foreground/80">
					Link the app that uses it below. Nothing else on this server can connect to it.
				</p>
			</div>
		{/if}

		{#if canLink}
			<div class="flex flex-col gap-2 sm:flex-row sm:items-center">
				<Select.Root type="single" bind:value={selectedAppId} disabled={candidates.length === 0}>
					<Select.Trigger class="w-full sm:w-72" aria-label="App to link">
						{selectedApp?.name ??
							(candidates.length === 0 ? 'Every eligible app has access' : 'Choose an app to link')}
					</Select.Trigger>
					<Select.Content>
						{#each candidates as app (app.id)}
							<Select.Item value={app.id}>{app.name}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<Button size="sm" onclick={handleLink} disabled={!selectedApp || linking}>
					<LinkIcon class="size-3.5" />
					{linking ? 'Linking…' : 'Link app'}
				</Button>
			</div>
		{:else}
			<p class="text-xs text-muted-foreground/70">
				Only admins can link and unlink databases: it changes which apps can reach them.
			</p>
		{/if}
		<p class="text-xs text-muted-foreground/70">
			Linking takes effect immediately; the app keeps running. Compose apps can't be linked (they
			run on their own compose network).
		</p>
	{/if}
</div>

<Dialog.Root bind:open={unlinkOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Unlink {unlinkTarget?.name}?</Dialog.Title>
			<Dialog.Description>
				{unlinkTarget?.name} loses its connection to {databaseName} right away. Open connections are cut,
				and requests that need the database will fail until you link it again.
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
