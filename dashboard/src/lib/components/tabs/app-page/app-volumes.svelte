<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import type { App } from '$lib/api';

	interface Props {
		app: App;
		volumeContainerPath: string;
		volumeError: string;
		onAdd: () => void;
		onRemove: (path: string) => void;
	}

	let { app, volumeContainerPath = $bindable(''), volumeError, onAdd, onRemove }: Props = $props();
</script>

<div class="space-y-6 pt-4">
	<p class="text-sm text-muted-foreground">
		Persistent volumes are bind-mounted from the host into the container. Changes take effect on the
		next deploy.
	</p>

	{#if volumeError}
		<ErrorAlert title="Error" description={volumeError} />
	{/if}

	<!-- Current volumes list -->
	{#if app.volumes.length > 0}
		<div class="rounded-md border">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b bg-muted/50">
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Container path</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Host path</th>
						<th class="w-16 px-4 py-2"></th>
					</tr>
				</thead>
				<tbody>
					{#each app.volumes as vol (vol.container_path)}
						<tr class="border-b last:border-0">
							<td class="px-4 py-2 font-mono">{vol.container_path}</td>
							<td class="px-4 py-2 font-mono text-muted-foreground">{vol.host_path}</td>
							<td class="px-4 py-2">
								<Button
									variant="ghost"
									size="sm"
									class="text-destructive hover:text-destructive"
									onclick={() => onRemove(vol.container_path)}
								>
									Remove
								</Button>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No volumes configured</p>
			<p class="mt-1 text-sm text-muted-foreground/60">
				Add a volume below to persist data between deploys
			</p>
		</div>
	{/if}

	<!-- Add volume form -->
	<div class="grid gap-3">
		<Label for="volume-path">Add volume</Label>
		<div class="flex gap-3">
			<Input
				id="volume-path"
				type="text"
				bind:value={volumeContainerPath}
				placeholder="/data"
				class="font-mono"
			/>
			<Button onclick={onAdd}>Add</Button>
		</div>
		<p class="text-xs text-muted-foreground">
			The host directory will be created automatically at <span class="font-mono"
				>$TOND_DATA_DIR/volumes/{app.name}/&lt;path&gt;</span
			>.
		</p>
	</div>
</div>
