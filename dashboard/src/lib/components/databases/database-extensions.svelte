<!--
  PostgreSQL extensions: the ones this image offers, which are enabled, and a
  button to enable one (CREATE EXTENSION IF NOT EXISTS, run by the server).
-->
<script lang="ts">
	import {
		enableDatabaseExtension,
		listDatabaseExtensions,
		type DatabaseExtension
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';

	interface Props {
		dbId: string;
		dbName: string;
		running: boolean;
		/** db:write */
		canEnable: boolean;
	}

	let { dbId, dbName, running, canEnable }: Props = $props();

	let extensions = $state<DatabaseExtension[]>([]);
	let loading = $state(false);
	let loadError = $state('');

	async function load() {
		loading = true;
		loadError = '';
		try {
			extensions = await listDatabaseExtensions(dbId);
		} catch (e: any) {
			loadError = e.message ?? 'Could not read the extensions';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void dbId;
		if (running) void load();
	});

	const enable = useAction((name: string) => enableDatabaseExtension(dbId, name), {
		key: (name) => name,
		success: (r) => r.message,
		onSuccess: load
	});
</script>

<section class="max-w-2xl space-y-3" aria-labelledby="db-ext-heading" data-testid="db-extensions">
	<div>
		<h2 id="db-ext-heading" class="text-sm font-semibold">Extensions</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			Enable an extension in {dbName}'s database. Enabling is permanent until you drop it with
			<code class="font-mono text-xs">DROP EXTENSION</code>.
		</p>
	</div>

	{#if !running}
		<p class="text-sm text-muted-foreground">Start the database to see and enable extensions.</p>
	{:else if loading && extensions.length === 0}
		<p class="text-xs text-muted-foreground">Reading the extensions…</p>
	{:else if loadError}
		<InlineError message={loadError}>
			{#snippet actions()}
				<Button size="sm" variant="outline" onclick={load}>Try again</Button>
			{/snippet}
		</InlineError>
	{:else}
		<ul class="divide-y divide-border overflow-hidden rounded border border-border">
			{#each extensions as ext (ext.name)}
				<li class="flex flex-col gap-2 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between">
					<div class="min-w-0">
						<div class="flex flex-wrap items-center gap-2">
							<span class="font-mono text-sm">{ext.name}</span>
							{#if ext.installed_version}
								<Badge variant="outline" class="border-success/40 text-[10px] text-success">
									Enabled · {ext.installed_version}
								</Badge>
							{:else if !ext.available}
								<Badge variant="outline" class="text-[10px]">Not in this image</Badge>
							{/if}
						</div>
						{#if ext.comment}
							<p class="truncate text-xs text-muted-foreground" title={ext.comment}>
								{ext.comment}
							</p>
						{/if}
					</div>
					{#if !ext.installed_version && ext.available && canEnable}
						<Button
							size="sm"
							variant="outline"
							class="shrink-0 self-start sm:self-auto"
							disabled={enable.pending}
							onclick={() => enable.run(ext.name)}
						>
							{enable.isPending(ext.name) ? 'Enabling…' : 'Enable'}
						</Button>
					{/if}
				</li>
			{/each}
		</ul>
		<SuccessAlert message={enable.success} ondismiss={enable.clear} timeout={5000} />
		<InlineError message={enable.error} ondismiss={enable.clear} />
	{/if}
</section>
