<script lang="ts">
	import { listAuditLog, type AuditLogEntry, type App } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import Activity from '~icons/ri/pulse-line';
	import ActivityFilterPanel from '$lib/components/activity/activity-filter-panel.svelte';
	import ActivityLogTable from '$lib/components/activity/activity-log-table.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { useAction } from '$lib/action.svelte';
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

	const refresh = useAction(refreshEntries);

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
	<PageHeader title="Activity" description={`${entries.length} events`}>
		{#snippet actions()}
			<Button
				variant="outline"
				size="sm"
				onclick={() => refresh.run()}
				disabled={refresh.pending}
				aria-busy={refresh.pending}
			>
				{refresh.pending ? 'Refreshing…' : 'Refresh'}
			</Button>
		{/snippet}
	</PageHeader>

	<InlineError message={error} ondismiss={() => (error = '')} />

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
			<Activity class="mb-4 h-12 w-12 text-muted-foreground" />
			<p class="font-medium">No activity yet</p>
			<p class="mt-2 text-sm text-muted-foreground">Actions will be recorded here</p>
		</div>
	{:else}
		<ActivityLogTable {entries} {apps} />
	{/if}
</div>
