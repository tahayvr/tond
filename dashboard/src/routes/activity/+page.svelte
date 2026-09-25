<script lang="ts">
	import { listAuditLog, type AuditLogEntry, type App } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import Activity from '~icons/ri/pulse-line';
	import ActivityFilterPanel from '$lib/components/activity/activity-filter-panel.svelte';
	import ActivityLogTable from '$lib/components/activity/activity-log-table.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	type ActivityPageData = {
		entries: AuditLogEntry[];
		apps: App[];
		initialFilterAppId: string;
		initialFilterAction: string;
		initialFilterLimit: number;
	};

	let entries = $state<AuditLogEntry[]>([]);
	let apps = $state<App[]>([]);
	let error = $state('');

	// Filter state
	let filterAppId = $state('');
	let filterAction = $state('');
	let filterLimit = $state(50);

	async function refreshEntries() {
		error = '';
		try {
			entries = await listAuditLog({ app_id: filterAppId || undefined, limit: filterLimit });
			// Client-side filtering for action type since API doesn't support it yet
			if (filterAction) {
				entries = entries.filter((e) => e.action === filterAction);
			}
		} catch (e: any) {
			error = e.message;
		}
	}

	$effect(() => {
		const routeData = data as ActivityPageData;
		apps = routeData.apps;
		entries = routeData.entries;
		filterAppId = routeData.initialFilterAppId;
		filterAction = routeData.initialFilterAction;
		filterLimit = routeData.initialFilterLimit;
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex h-14 items-center justify-between">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Activity</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{entries.length} EVENTS_LOGGED
			</p>
		</div>
		<Button variant="outline" size="sm" onclick={refreshEntries}>Refresh</Button>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-destructive">
			{error}
		</div>
	{/if}

	<ActivityFilterPanel
		{apps}
		bind:filterAppId
		bind:filterAction
		bind:filterLimit
		onApply={refreshEntries}
	/>

	{#if entries.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Activity class="mb-4 h-12 w-12 text-muted-foreground/40" />
			<p class="font-mono text-[11px] tracking-widest text-muted-foreground/60 uppercase">
				NO_EVENTS_LOGGED
			</p>
			<p class="mt-2 text-sm text-muted-foreground/70">Actions will be recorded here</p>
		</div>
	{:else}
		<ActivityLogTable {entries} {apps} />
	{/if}
</div>
