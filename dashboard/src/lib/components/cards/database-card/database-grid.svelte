<script lang="ts">
	import DatabaseCard from './database-card.svelte';
	import type { ManagedDatabase } from '$lib/api';

	let { databases }: { databases: ManagedDatabase[] } = $props();
</script>

<div class="grid grid-cols-1 items-stretch gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
	{#each databases as db (db.id)}
		<DatabaseCard {db} />
	{/each}
</div>
