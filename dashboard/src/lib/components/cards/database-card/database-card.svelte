<script lang="ts">
	import { goto } from '$app/navigation';
	import { type ManagedDatabase } from '$lib/api';
	import {
		getUiContext,
		setUiContext,
		databasesContext,
		databaseContext
	} from '$lib/ui-context.svelte';
	import * as Card from '$lib/components/ui/card/index.js';
	import { dbLogoUrl } from '$lib/components/brand-logos/db-logos.svelte';
	import DatabaseIcon from '@lucide/svelte/icons/database';

	let { db }: { db: ManagedDatabase } = $props();

	let ctx = $derived(getUiContext());
	let selected = $derived(ctx.type === 'database' && ctx.database.id === db.id);

	let logoUrl = $derived(dbLogoUrl(db.db_type));
	let imgError = $state(false);

	function handleSelect() {
		if (selected) {
			setUiContext(databasesContext());
		} else {
			setUiContext(databaseContext(db));
		}
		goto(`/databases/${db.id}`);
	}
</script>

<button
	type="button"
	class="block h-full w-full text-left hover:cursor-pointer"
	onclick={handleSelect}
>
	<Card.Root
		class="flex h-full w-full flex-col rounded-sm border-none p-6 transition-colors hover:border {selected
			? 'invert'
			: ''}"
	>
		<Card.Header class="p-0">
			<Card.Description class="text-xs uppercase">id: {db.id.slice(0, 8)}</Card.Description>
			<Card.Title class="text-base uppercase">{db.name}</Card.Title>
		</Card.Header>
		<Card.Content class="grow p-0">
			<div class="flex h-full flex-col justify-center gap-3 py-6">
				<!-- Engine logo / fallback badge -->
				<div class="flex items-center justify-center">
					{#if logoUrl && !imgError}
						<img
							src={logoUrl}
							alt={db.db_type}
							class="h-10 w-10 object-contain opacity-80"
							onerror={() => (imgError = true)}
						/>
					{:else}
						<DatabaseIcon class="h-10 w-10 text-muted-foreground/40" />
					{/if}
				</div>
				<!-- Status row -->
				<div class="flex items-center justify-center gap-2">
					<span
						class="inline-block size-2 rounded-full {db.status === 'running'
							? 'bg-accent'
							: 'bg-muted-foreground/40'}"
					></span>
					<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
						{db.status}
					</span>
				</div>
				{#if db.host_port}
					<p class="text-center font-mono text-[10px] text-muted-foreground/70 uppercase">
						PORT: {db.host_port}
					</p>
				{/if}
			</div>
		</Card.Content>
		<Card.Footer class="w-full flex-col gap-3 p-0">
			<div class="flex w-full items-center justify-between text-xs text-muted-foreground">
				<span>Created</span>
				<span>{new Date(db.created_at).toLocaleDateString()}</span>
			</div>
		</Card.Footer>
	</Card.Root>
</button>
