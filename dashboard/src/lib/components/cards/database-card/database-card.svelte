<script lang="ts">
	import { type ManagedDatabase } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { dbLogoUrl } from '$lib/components/brand-logos/db-logos.svelte';
	import DatabaseIcon from '~icons/ri/database-2-line';

	let { db }: { db: ManagedDatabase } = $props();

	let logoUrl = $derived(dbLogoUrl(db.db_type));
	let imgError = $state(false);
</script>

<a href={`/databases/${db.id}`} class="block h-full rounded-sm" data-sveltekit-preload-data="hover">
	<Card.Root
		class="flex h-full w-full flex-col gap-3 rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="gap-2 p-0">
			<div class="flex items-start justify-between gap-2">
				<div class="flex min-w-0 items-center gap-2">
					{#if logoUrl && !imgError}
						<img
							src={logoUrl}
							alt=""
							class="h-6 w-6 shrink-0 object-contain"
							onerror={() => (imgError = true)}
						/>
					{:else}
						<DatabaseIcon aria-hidden="true" class="h-6 w-6 shrink-0 text-muted-foreground" />
					{/if}
					<Card.Title class="min-w-0 truncate text-base">{db.name}</Card.Title>
				</div>
				<StatusBadge status={db.status} kind="database" size="sm" />
			</div>
			<Card.Description class="text-xs">
				{db.db_type}
			</Card.Description>
		</Card.Header>
		<Card.Content
			class="flex grow items-end justify-between gap-2 p-0 text-xs text-muted-foreground"
		>
			<span>{db.host_port ? `Port ${db.host_port}` : 'Internal only'}</span>
			<span>Created {new Date(db.created_at).toLocaleDateString()}</span>
		</Card.Content>
	</Card.Root>
</a>
