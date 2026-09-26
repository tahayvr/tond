<!--
  Restore a backup into a NEW database (same engine and version, its own
  credentials and port). The original database is never touched: use it to
  test a backup or pull old rows out of it.
-->
<script lang="ts">
	import {
		listDatabases,
		restoreDatabaseToNew,
		type RestoredDatabase,
		type Snapshot
	} from '$lib/api';
	import { restoreName, validDatabaseName } from '$lib/database-ops';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';

	interface Props {
		open: boolean;
		snapshot: Snapshot | null;
		/** The database whose data is restored */
		source: string;
		/** Names already in use */
		taken?: string[];
		onrestored?: (result: RestoredDatabase) => void;
	}

	let { open = $bindable(false), snapshot, source, taken = [], onrestored }: Props = $props();

	let name = $state('');
	// Names in use: from the caller, plus the server's list once the dialog opens.
	let existing = $state<string[]>([]);
	let allTaken = $derived([source, ...taken, ...existing]);
	let restoring = $state(false);
	let error = $state('');

	$effect(() => {
		if (!open || !snapshot) return;
		const created = new Date(snapshot.created_at);
		name = restoreName(source, created, [source, ...taken]);
		error = '';
		let cancelled = false;
		listDatabases()
			.then((dbs) => {
				if (cancelled) return;
				existing = dbs.map((d) => d.name);
				if (existing.includes(name)) name = restoreName(source, created, allTaken);
			})
			.catch(() => {});
		return () => {
			cancelled = true;
		};
	});

	let nameError = $derived(
		!name.trim()
			? 'Enter a name'
			: !validDatabaseName(name.trim())
				? 'Letters, digits and hyphens only'
				: allTaken.includes(name.trim())
					? 'A database with this name exists'
					: ''
	);

	async function handleRestore() {
		if (!snapshot || nameError) return;
		restoring = true;
		error = '';
		try {
			const result = await restoreDatabaseToNew(snapshot.id, { source, name: name.trim() });
			open = false;
			onrestored?.(result);
		} catch (e: any) {
			error = e.message ?? 'The restore failed. The original database was not changed.';
		} finally {
			restoring = false;
		}
	}

	function when(iso: string): string {
		return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Restore into a new database</Dialog.Title>
			<Dialog.Description>
				{#if snapshot}
					Creates a new database with the data {source} had on
					<strong>{when(snapshot.created_at)}</strong>. {source} keeps running and is not changed.
				{/if}
			</Dialog.Description>
		</Dialog.Header>

		<form
			class="grid gap-3 py-2"
			onsubmit={(e) => {
				e.preventDefault();
				handleRestore();
			}}
		>
			<div class="grid gap-2">
				<Label for="restore-new-name">Name of the new database</Label>
				<Input
					id="restore-new-name"
					bind:value={name}
					autocomplete="off"
					spellcheck={false}
					class="font-mono"
					aria-invalid={!!nameError}
					disabled={restoring}
				/>
				{#if nameError && name}
					<p class="text-xs text-destructive">{nameError}</p>
				{:else}
					<p class="text-xs text-muted-foreground">
						Same engine and version as {source}, with new credentials and its own port. Delete it when
						you're done.
					</p>
				{/if}
			</div>
			<InlineError message={error} />
			<Dialog.Footer>
				<Button type="button" variant="outline" disabled={restoring} onclick={() => (open = false)}>
					Cancel
				</Button>
				<Button type="submit" disabled={!!nameError || restoring} aria-busy={restoring}>
					{restoring ? 'Restoring… this can take a few minutes' : 'Create and restore'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
