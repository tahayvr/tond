<script lang="ts">
	import { createProjectPreview } from '$lib/api';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import type { ProjectPreview } from '$lib/api';

	let {
		open = $bindable(false),
		projectId,
		oncreated
	}: {
		open: boolean;
		projectId: string;
		oncreated?: (preview: ProjectPreview) => void;
	} = $props();

	let ttlHours = $state(24);
	let error = $state('');
	let submitting = $state(false);

	function reset() {
		ttlHours = 24;
		error = '';
		submitting = false;
	}

	async function handleSubmit() {
		submitting = true;
		error = '';
		try {
			const preview = await createProjectPreview(projectId, ttlHours);
			open = false;
			reset();
			oncreated?.(preview);
		} catch (e: any) {
			error = e.message ?? 'Failed to create project preview';
			submitting = false;
		}
	}
</script>

<Dialog.Root
	bind:open
	onOpenChange={(isOpen) => {
		if (!isOpen) reset();
	}}
>
	<Dialog.Content class="sm:max-w-sm">
		<Dialog.Header>
			<Dialog.Title>Create Preview Environment</Dialog.Title>
			<Dialog.Description>
				Spins up one preview deployment per member app under a shared URL prefix.
			</Dialog.Description>
		</Dialog.Header>
		<form
			onsubmit={(e) => {
				e.preventDefault();
				handleSubmit();
			}}
			class="grid gap-4 py-2"
		>
			<div class="grid gap-2">
				<Label for="preview-ttl" class="uppercase">TTL (hours)</Label>
				<Input
					id="preview-ttl"
					type="number"
					min="1"
					max="168"
					bind:value={ttlHours}
					disabled={submitting}
				/>
				<p class="text-xs text-muted-foreground">
					Preview will auto-expire after this many hours (max 168 / 7 days).
				</p>
			</div>

			{#if error}
				<div class="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</div>
			{/if}

			<Dialog.Footer>
				<Button
					variant="outline"
					type="button"
					onclick={() => (open = false)}
					disabled={submitting}
				>
					Cancel
				</Button>
				<Button type="submit" disabled={submitting}>
					{submitting ? 'Creating…' : 'Create Preview'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
