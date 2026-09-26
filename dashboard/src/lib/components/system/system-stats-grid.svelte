<script lang="ts">
	import Server from '~icons/ri/server-line';
	import Cpu from '~icons/ri/cpu-line';
	import HardDrive from '~icons/ri/hard-drive-3-line';
	import Box from '~icons/ri/box-3-line';
	import ResourceGauge from '$lib/components/system/resource-gauge.svelte';
	import MetricCard from '$lib/components/system/metric-card.svelte';
	import type { SystemMetrics } from '$lib/api';

	interface Props {
		metrics: SystemMetrics;
	}

	let { metrics }: Props = $props();

	function fmt(bytes: number): string {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
		return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
	}
</script>

<div class="space-y-6">
	<!-- Resource gauges -->
	<div class="grid gap-4 sm:grid-cols-3">
		<ResourceGauge value={metrics.cpu_percent} label="CPU_USAGE" icon={Cpu} color="bg-success/60" />

		<ResourceGauge
			value={metrics.mem_percent}
			label="MEMORY_USAGE"
			icon={Server}
			color="bg-accent/60"
			detail="{fmt(metrics.mem_used)} / {fmt(metrics.mem_total)}"
		/>

		<ResourceGauge
			value={metrics.disk_percent}
			label="DISK_USAGE"
			icon={HardDrive}
			color="bg-warn/60"
			detail="{fmt(metrics.disk_used)} / {fmt(metrics.disk_total)} on {metrics.disk_mount || '/'}"
		/>
	</div>

	<!-- App & container summary -->
	<div class="grid gap-4 sm:grid-cols-3">
		<MetricCard
			value={metrics.containers_running}
			label="CONTAINERS"
			icon={Box}
			detail="Run by Tond right now"
		/>

		<MetricCard value={metrics.apps_total} label="TOND_APPS" detail="Total registered" />

		<MetricCard
			value={metrics.apps_running}
			label="RUNNING"
			detail="Apps running"
			healthy={metrics.apps_running > 0}
			healthyDetail="{metrics.apps_running} / {metrics.apps_total} healthy"
		/>
	</div>

	<p class="font-mono text-[10px] text-muted-foreground">
		SNAPSHOT: {new Date(metrics.collected_at).toLocaleString()}
	</p>
</div>
