<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import LockIcon from '~icons/ri/lock-line';
	import type { App } from '$lib/api';

	interface Props {
		app: App;
		volumeContainerPath: string;
		volumeName: string;
		/** Only admins may add or remove volumes */
		canEdit: boolean;
		volumeError: string;
		onAdd: () => void;
		onRemove: (path: string) => void;
	}

	let {
		app,
		volumeContainerPath = $bindable(''),
		volumeName = $bindable(''),
		canEdit,
		volumeError,
		onAdd,
		onRemove
	}: Props = $props();

	/** Mirrors the server's default: "/data/cache" → "data_cache". */
	function defaultName(path: string): string {
		const name = path
			.replace(/^\/+|\/+$/g, '')
			.replace(/[^A-Za-z0-9._-]/g, '_')
			.replace(/^\.+/, '');
		return name || 'data';
	}

	let previewName = $derived(volumeName.trim() || defaultName(volumeContainerPath.trim()));
	let volumeDir = $derived.by(() => {
		const existing = app.volumes[0]?.host_path;
		if (existing) return existing.slice(0, existing.lastIndexOf('/'));
		return `/opt/tond/data/volumes/${app.name}`;
	});
</script>

<div class="space-y-6 pt-4">
	<p class="text-sm text-muted-foreground">
		Persistent volumes keep data between deploys. Each volume is a directory inside this app's
		volume folder <span class="font-mono">{volumeDir}</span>; containers can't mount any other host
		path. Changes take effect on the next deploy.
	</p>

	{#if volumeError}
		<ErrorAlert title="Volume not saved" description={volumeError} />
	{/if}

	<!-- Current volumes list -->
	{#if app.volumes.length > 0}
		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b bg-muted/50">
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Container path</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Name</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Host path</th>
						<th class="w-16 px-4 py-2"></th>
					</tr>
				</thead>
				<tbody>
					{#each app.volumes as vol (vol.container_path)}
						<tr class="border-b last:border-0">
							<td class="px-4 py-2 font-mono">{vol.container_path}</td>
							<td class="px-4 py-2 font-mono">{vol.name || '-'}</td>
							<td class="px-4 py-2 font-mono text-muted-foreground">{vol.host_path}</td>
							<td class="px-4 py-2">
								{#if canEdit}
									<Button
										variant="ghost"
										size="sm"
										class="text-destructive hover:text-destructive"
										onclick={() => onRemove(vol.container_path)}
									>
										Remove
									</Button>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No volumes configured</p>
			{#if canEdit}
				<p class="mt-1 text-sm text-muted-foreground/60">
					Add a volume below to persist data between deploys
				</p>
			{/if}
		</div>
	{/if}

	{#if canEdit}
		<!-- Add volume form -->
		<div class="grid gap-3">
			<Label>Add volume</Label>
			<div class="grid gap-3 sm:grid-cols-[1fr_12rem_auto]">
				<div class="grid gap-1">
					<Label for="volume-path" class="text-xs text-muted-foreground">Container path</Label>
					<Input
						id="volume-path"
						type="text"
						bind:value={volumeContainerPath}
						placeholder="/data"
						class="font-mono"
					/>
				</div>
				<div class="grid gap-1">
					<Label for="volume-name" class="text-xs text-muted-foreground">Name (optional)</Label>
					<Input
						id="volume-name"
						type="text"
						bind:value={volumeName}
						placeholder={defaultName(volumeContainerPath.trim())}
						class="font-mono"
					/>
				</div>
				<div class="flex items-end">
					<Button onclick={onAdd} disabled={!volumeContainerPath.trim()}>Add</Button>
				</div>
			</div>
			<p class="text-xs text-muted-foreground">
				Stored at <span class="font-mono">{volumeDir}/{previewName}</span>. Names may use letters,
				digits, <span class="font-mono">.</span>, <span class="font-mono">_</span> and
				<span class="font-mono">-</span>.
			</p>
		</div>
	{:else}
		<div
			class="flex items-start gap-2 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground"
		>
			<LockIcon class="mt-0.5 size-4 shrink-0" />
			<p>Only admins can add or remove volumes, because volumes are directories on the server.</p>
		</div>
	{/if}
</div>
