<script lang="ts">
	import { deleteDatabase, type ManagedDatabase } from '$lib/api';
	import { goto } from '$app/navigation';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, databasesContext } from '$lib/ui-context.svelte';
	import Database from '@lucide/svelte/icons/database';
	import DatabaseGrid from '$lib/components/cards/database-card/database-grid.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let databases = $derived(data.databases as ManagedDatabase[]);
	let error = $state('');

	// Delete dialog state
	let deleteDialogOpen = $state(false);
	let dbToDelete = $state<ManagedDatabase | null>(null);

	let runningCount = $derived(databases.filter((d) => d.status === 'running').length);

	async function handleConfirmDelete() {
		if (!dbToDelete) return;
		const deletingId = dbToDelete.id;
		try {
			await deleteDatabase(deletingId);
			databases = databases.filter((db) => db.id !== deletingId);
		} catch (e: any) {
			error = e.message ?? 'Failed to delete database';
		} finally {
			dbToDelete = null;
			deleteDialogOpen = false;
		}
	}

	$effect(() => {
		setUiContext(databasesContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex h-14 items-center justify-between">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Databases</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{runningCount}/{databases.length} DATASTORES_ACTIVE
			</p>
		</div>
		<Button onclick={() => goto('/databases/new')}>New Database</Button>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-destructive">
			{error}
		</div>
	{/if}

	{#if databases.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Database class="mb-4 h-12 w-12 text-muted-foreground/40" />
			<p class="font-mono text-[11px] tracking-widest text-muted-foreground/60 uppercase">
				NO_DATASTORES_FOUND
			</p>
			<p class="mt-2 text-sm text-muted-foreground/70">Create a database to get started</p>
		</div>
	{:else}
		<DatabaseGrid {databases} />
	{/if}
</div>

<ConfirmDeleteDialog
	bind:open={deleteDialogOpen}
	title={dbToDelete ? `TERMINATE "${dbToDelete.name}"?` : 'TERMINATE DATASTORE?'}
	description="This will stop the database container and permanently remove all data. This action cannot be undone."
	confirmation={dbToDelete?.name ?? ''}
	onconfirm={handleConfirmDelete}
/>
