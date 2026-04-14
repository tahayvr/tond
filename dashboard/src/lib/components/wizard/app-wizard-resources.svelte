<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';

	interface Volume {
		host_path: string;
		container_path: string;
	}

	interface Props {
		memoryLimit: number | null;
		memoryUnit: 'mb' | 'gb';
		cpuLimit: number | null;
		volumes: Volume[];
		newVolumePath: string;
		onAddVolume: () => void;
		onRemoveVolume: (index: number) => void;
	}

	let {
		memoryLimit = $bindable(null),
		memoryUnit = $bindable('mb'),
		cpuLimit = $bindable(null),
		volumes = $bindable([]),
		newVolumePath = $bindable(''),
		onAddVolume,
		onRemoveVolume
	}: Props = $props();
</script>

<div class="space-y-6">
	<div class="grid gap-4 sm:grid-cols-2">
		<div class="space-y-2">
			<Label>Memory Limit</Label>
			<div class="flex gap-2">
				<Input type="number" bind:value={memoryLimit} placeholder="Unlimited" min={1} />
				<Select.Root type="single" bind:value={memoryUnit}>
					<Select.Trigger class="w-24">
						{memoryUnit.toUpperCase()}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="mb">MB</Select.Item>
						<Select.Item value="gb">GB</Select.Item>
					</Select.Content>
				</Select.Root>
			</div>
			<p class="text-xs text-muted-foreground">Leave empty for unlimited memory</p>
		</div>

		<div class="space-y-2">
			<Label for="cpu-limit">CPU Limit</Label>
			<Input
				id="cpu-limit"
				type="number"
				bind:value={cpuLimit}
				placeholder="Unlimited"
				min={0.1}
				max={32}
				step={0.1}
			/>
			<p class="text-xs text-muted-foreground">CPU cores (e.g., 0.5 = half a core)</p>
		</div>
	</div>

	<Separator class="bg-border" />

	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<h4 class="font-mono text-sm tracking-wider uppercase">Persistent Volumes</h4>
		</div>

		{#if volumes.length > 0}
			<div class="space-y-2">
				{#each volumes as volume, index (index)}
					<div class="flex items-center gap-2 rounded border border-border bg-muted p-3">
						<div class="flex-1 font-mono text-xs">
							<div class="text-muted-foreground">Container: {volume.container_path}</div>
							<div class="text-muted-foreground/70">Host: {volume.host_path}</div>
						</div>
						<Button
							variant="ghost"
							size="sm"
							onclick={() => onRemoveVolume(index)}
							class="text-destructive hover:text-destructive/80"
						>
							Remove
						</Button>
					</div>
				{/each}
			</div>
		{/if}

		<div class="flex gap-2">
			<Input type="text" bind:value={newVolumePath} placeholder="/app/data" class="font-mono" />
			<Button variant="outline" onclick={onAddVolume} disabled={!newVolumePath.trim()}>
				Add Volume
			</Button>
		</div>
		<p class="text-xs text-muted-foreground">
			Mount host directories into the container. Host paths are auto-generated.
		</p>
	</div>
</div>
