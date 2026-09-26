<script lang="ts">
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { goto } from '$app/navigation';
	import {
		listAuditLog,
		verifyAuditLog,
		type AuditLogEntry,
		type AuditVerifyReport
	} from '$lib/api';
	import { filtersToQuery, filtersToSearch, type ActivityFilters } from '$lib/activity-filters';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import ActivityIcon from '~icons/ri/pulse-line';
	import ShieldCheck from '~icons/ri/shield-check-line';
	import ShieldCross from '~icons/ri/shield-cross-line';
	import ActivityFilterPanel from '$lib/components/activity/activity-filter-panel.svelte';
	import ActivityLogTable from '$lib/components/activity/activity-log-table.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let entries = $state<AuditLogEntry[]>([]);
	let nextBefore = $state<number | null>(null);
	let filters = $derived(data.filters as ActivityFilters);
	let error = $state('');
	let applying = $state(false);
	let loadingMore = $state(false);

	let verifying = $state(false);
	let report = $state<AuditVerifyReport | null>(null);
	let verifyError = $state('');

	$effect(() => {
		entries = data.entries as AuditLogEntry[];
		nextBefore = data.nextBefore as number | null;
	});

	const knownActors = $derived([...new Set(entries.map((e) => e.actor_label))].sort());

	async function handleApply(next: ActivityFilters) {
		error = '';
		applying = true;
		try {
			await goto(`/activity${filtersToSearch(next)}`, {
				replaceState: true,
				keepFocus: true,
				noScroll: true
			});
		} catch (e: any) {
			error = e.message;
		} finally {
			applying = false;
		}
	}

	async function handleLoadMore() {
		if (nextBefore === null) return;
		loadingMore = true;
		error = '';
		try {
			const page = await listAuditLog(filtersToQuery(filters, nextBefore));
			entries = [...entries, ...page.entries];
			nextBefore = page.next_before;
		} catch (e: any) {
			error = e.message;
		} finally {
			loadingMore = false;
		}
	}

	async function handleVerify() {
		verifying = true;
		verifyError = '';
		report = null;
		try {
			report = await verifyAuditLog();
		} catch (e: any) {
			verifyError = e.message;
		} finally {
			verifying = false;
		}
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="Activity"
		description="Every change and security event, with who did it. The log is append-only."
	>
		{#snippet actions()}
			<Button variant="outline" size="sm" onclick={handleVerify} disabled={verifying}>
				<ShieldCheck class="mr-1.5 size-4" />
				{verifying ? 'Checking…' : 'Verify integrity'}
			</Button>
		{/snippet}
	</PageHeader>

	{#if report}
		{#if report.ok}
			<div
				class="flex items-start gap-2 rounded-md border border-success/40 bg-success/10 p-3 text-sm"
				role="status"
			>
				<ShieldCheck class="mt-0.5 size-4 shrink-0 text-success" />
				<p>
					The log is intact: {report.checked} entries checked and none were changed or removed{report.checkpoint_seq >
					0
						? `. Entries before #${report.checkpoint_seq + 1} were removed by the retention policy`
						: ''}.
				</p>
			</div>
		{:else}
			<div
				class="flex items-start gap-2 rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive"
				role="alert"
			>
				<ShieldCross class="mt-0.5 size-4 shrink-0" />
				<p>
					The log was tampered with at entry #{report.broken_at}: {report.problem}. Treat the server
					as compromised and check who has root access.
				</p>
			</div>
		{/if}
	{/if}
	<InlineError message={verifyError} />

	<ActivityFilterPanel {filters} {knownActors} loading={applying} onApply={handleApply} />

	<InlineError message={error} />

	{#if entries.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-dashed border-border py-16 text-center"
		>
			<ActivityIcon class="mb-3 size-10 text-muted-foreground" />
			<p class="font-medium">No matching events</p>
			<p class="mt-1 text-sm text-muted-foreground">
				Try a different filter. Deploys, sign-ins and changes show up here as they happen.
			</p>
		</div>
	{:else}
		<ActivityLogTable {entries} onFilterActor={(actor) => handleApply({ ...filters, actor })} />
		<div class="flex justify-center">
			{#if nextBefore !== null}
				<Button variant="outline" size="sm" onclick={handleLoadMore} disabled={loadingMore}>
					{loadingMore ? 'Loading…' : 'Load older events'}
				</Button>
			{:else}
				<p class="text-xs text-muted-foreground">That's everything that matches.</p>
			{/if}
		</div>
	{/if}
</div>
