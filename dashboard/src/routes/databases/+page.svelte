<script lang="ts">
	import { can } from '$lib/auth.svelte';
	import type { ManagedDatabase } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, databasesContext } from '$lib/ui-context.svelte';
	import Database from '~icons/ri/database-2-line';
	import Plus from '~icons/ri/add-line';
	import DatabaseGrid from '$lib/components/cards/database-card/database-grid.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let databases = $derived(data.databases as ManagedDatabase[]);
	let runningCount = $derived(databases.filter((d) => d.status === 'running').length);

	$effect(() => {
		setUiContext(databasesContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="Databases"
		description={databases.length ? `${runningCount} of ${databases.length} running` : undefined}
	>
		{#snippet actions()}
			{#if can('db:write')}<Button href="/databases/new">
					<Plus class="mr-1 h-4 w-4" />
					New database
				</Button>{/if}
		{/snippet}
	</PageHeader>

	{#if databases.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card px-4 py-16 text-center"
		>
			<Database aria-hidden="true" class="mb-4 h-12 w-12 text-muted-foreground" />
			<p class="font-medium">No databases yet</p>
			<p class="mt-2 text-sm text-muted-foreground">
				PostgreSQL, MySQL, MariaDB, Redis and more, with backups and connection strings.
			</p>
			<Button class="mt-4" href="/databases/new">
				<Plus class="mr-1 h-4 w-4" />
				Create database
			</Button>
		</div>
	{:else}
		<DatabaseGrid {databases} />
	{/if}
</div>
