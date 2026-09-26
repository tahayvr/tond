<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import LockIcon from '~icons/ri/lock-line';
	import type { VolumeInput } from '$lib/api';
	import { defaultMemoryCap, formatBytes } from '$lib/wizard-detect';

	interface Props {
		appName: string;
		/** Only admins may configure volumes */
		canEditVolumes: boolean;
		memoryLimit: number | null;
		memoryUnit: 'mb' | 'gb';
		cpuLimit: number | null;
		volumes: VolumeInput[];
		newVolumePath: string;
		onAddVolume: () => void;
		onRemoveVolume: (index: number) => void;
		/** The server's total memory, for explaining the default cap (null = unknown). */
		hostMemoryBytes?: number | null;
	}

	let {
		appName,
		hostMemoryBytes = null,
		canEditVolumes,
		memoryLimit = $bindable(null),
		memoryUnit = $bindable('mb'),
		cpuLimit = $bindable(null),
		volumes = $bindable([]),
		newVolumePath = $bindable(''),
		onAddVolume,
		onRemoveVolume
	}: Props = $props();

	/** Mirrors the server's default volume name: "/data/cache" → "data_cache". */
	function volumeDir(v: VolumeInput): string {
		const name =
			v.name ||
			v.container_path
				.replace(/^\/+|\/+$/g, '')
				.replace(/[^A-Za-z0-9._-]/g, '_')
				.replace(/^\.+/, '') ||
			'data';
		return `/opt/tond/data/volumes/${appName || '<app>'}/${name}`;
	}
</script>

<div class="space-y-6">
	<div class="grid gap-4 sm:grid-cols-2">
		<div class="space-y-2">
			<Label>Memory Limit</Label>
			<div class="flex gap-2">
				<Input
					type="number"
					bind:value={memoryLimit}
					placeholder="Default cap (75% of RAM)"
					min={1}
				/>
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
			<p class="text-xs text-muted-foreground">
				Leave empty for no app-specific limit. Tond still caps every container at 75% of the
				server's memory{hostMemoryBytes
					? ` (${formatBytes(defaultMemoryCap(hostMemoryBytes))} on this server)`
					: ''}, so one app can't take the whole server down.
			</p>
		</div>

		<div class="space-y-2">
			<Label for="cpu-limit">CPU Limit</Label>
			<Input
				id="cpu-limit"
				type="number"
				bind:value={cpuLimit}
				placeholder="All cores"
				min={0.1}
				max={32}
				step={0.1}
			/>
			<p class="text-xs text-muted-foreground">
				CPU cores (0.5 = half a core). Leave empty to let the app use every core.
			</p>
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
							<div class="text-muted-foreground">Stored in: {volumeDir(volume)}</div>
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

		{#if canEditVolumes}
			<div class="flex gap-2">
				<Input type="text" bind:value={newVolumePath} placeholder="/app/data" class="font-mono" />
				<Button variant="outline" onclick={onAddVolume} disabled={!newVolumePath.trim()}>
					Add Volume
				</Button>
			</div>
			<p class="text-xs text-muted-foreground">
				Enter the path inside the container. Tond stores the data in this app's own folder on the
				server; other host paths can't be mounted.
			</p>
		{:else}
			<div
				class="flex items-start gap-2 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground"
			>
				<LockIcon class="mt-0.5 size-4 shrink-0" />
				<p>Only admins can configure volumes, because volumes are directories on the server.</p>
			</div>
		{/if}
	</div>
</div>
