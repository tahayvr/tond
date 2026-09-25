<script lang="ts">
	import { getSystemMetrics, type SystemMetrics } from '$lib/api';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import CpuIcon from '~icons/ri/cpu-line';
	import MemoryIcon from '~icons/ri/ram-2-line';
	import BoxIcon from '~icons/ri/box-3-line';
	import LayersIcon from '~icons/ri/stack-line';
	import AlertCircleIcon from '~icons/ri/error-warning-line';
	// import HardDriveIcon from '~icons/ri/hard-drive-3-line';
	// import Box from '~icons/ri/box-3-line';

	let metrics = $state<SystemMetrics | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	async function load() {
		try {
			metrics = await getSystemMetrics();
			error = null;
		} catch (e: any) {
			error = e.message || 'Failed to load metrics';
			console.error('Footer metrics error:', e);
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		load();
		const interval = setInterval(load, 30_000);
		return () => clearInterval(interval);
	});
</script>

{#if metrics}
	{@const m = metrics}
	<div class="flex items-center gap-4 text-xs text-muted-foreground">
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<button {...props} class="flex cursor-default items-center gap-1">
						<CpuIcon class="size-3.5" />
						<span class="font-medium tabular-nums">{Math.round(m.cpu_percent)}%</span>
					</button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<p>CPU Usage</p>
			</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<button {...props} class="flex cursor-default items-center gap-1">
						<MemoryIcon class="size-3.5" />
						<span class="font-medium tabular-nums">{Math.round(m.mem_percent)}%</span>
					</button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<p>RAM Usage</p>
			</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<button {...props} class="flex cursor-default items-center gap-1">
						<BoxIcon class="size-3.5" />
						<span class="font-medium tabular-nums">{m.containers_running}</span>
					</button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<p>Running Containers</p>
			</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<button {...props} class="flex cursor-default items-center gap-1">
						<LayersIcon class="size-3.5" />
						<span class="font-medium tabular-nums">{m.apps_running}</span>
					</button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<p>Running Apps</p>
			</Tooltip.Content>
		</Tooltip.Root>
	</div>
{:else if loading}
	<div class="flex items-center gap-3 text-xs text-muted-foreground/50">
		<span>Loading…</span>
	</div>
{:else if error}
	<div class="flex items-center gap-1 text-xs text-destructive">
		<AlertCircleIcon class="size-3.5" />
		<span>{error}</span>
	</div>
{/if}
