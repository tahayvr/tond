<script lang="ts">
	// Automatic disk cleanup: the disk percentage at which Tond prunes unused
	// images (beyond the kept releases) and then the build cache. 0 = off.
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { updateSettings, type ServerSettings } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import BroomIcon from '~icons/ri/eraser-line';

	interface Props {
		settings: ServerSettings;
		canEdit: boolean;
	}

	let { settings, canEdit }: Props = $props();

	let enabled = $state(true);
	let percent = $state(85);
	let saving = $state(false);
	let saved = $state(false);
	let error = $state('');

	$effect(() => {
		const value = settings.auto_prune_disk_percent ?? 85;
		enabled = value > 0;
		percent = value > 0 ? value : 85;
	});

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';
		saved = false;
		if (enabled && (!Number.isInteger(percent) || percent < 50 || percent > 99)) {
			error = 'The threshold must be a whole number between 50 and 99 percent.';
			return;
		}
		saving = true;
		try {
			const updated = await updateSettings({ auto_prune_disk_percent: enabled ? percent : 0 });
			const value = updated.auto_prune_disk_percent ?? 85;
			enabled = value > 0;
			if (value > 0) percent = value;
			saved = true;
			setTimeout(() => (saved = false), 3000);
		} catch (err: any) {
			error = err.message;
		} finally {
			saving = false;
		}
	}
</script>

<form
	class="rounded-lg border border-border bg-card p-4"
	onsubmit={handleSubmit}
	novalidate
	aria-labelledby="disk-cleanup-title"
>
	<div class="mb-1 flex items-center gap-2">
		<BroomIcon class="size-4 text-muted-foreground" />
		<h2 id="disk-cleanup-title" class="font-semibold">Disk cleanup</h2>
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		When the disk is this full, Tond removes images no app needs (it keeps every app's last three
		releases for rollback) and then Docker's build cache. Volumes and their data are never touched.
		Each cleanup is recorded in Activity, and an <em>Automatic disk cleanup</em> alert rule can notify
		you.
	</p>
	<div class="grid max-w-xl gap-4 sm:grid-cols-2">
		<label class="flex items-center gap-2 text-sm">
			<input
				type="checkbox"
				class="size-4 accent-primary"
				bind:checked={enabled}
				disabled={!canEdit}
			/>
			Clean up automatically
		</label>
		<div class="grid gap-1.5">
			<Label for="auto-prune-percent">When the disk is at least (%)</Label>
			<Input
				id="auto-prune-percent"
				type="number"
				min={50}
				max={99}
				bind:value={percent}
				disabled={!canEdit || !enabled}
			/>
		</div>
	</div>
	<InlineError message={error} />
	<div class="mt-4 flex items-center gap-3">
		{#if canEdit}
			<Button type="submit" size="sm" disabled={saving}>{saving ? 'Saving…' : 'Save'}</Button>
			{#if saved}<span class="text-xs text-success" role="status">Saved.</span>{/if}
		{:else}
			<p class="text-xs text-muted-foreground">Only admins can change this.</p>
		{/if}
	</div>
</form>
