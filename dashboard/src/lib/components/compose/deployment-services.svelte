<script lang="ts">
	// Per-service progress of a compose deployment: how each image was made
	// (built, pulled, kept for a rollback), the pinned tag and digest, and
	// where the service is (starting, ready, failed).
	import type { DeploymentService } from '$lib/api';
	import { deploymentServiceBadge, shortDigest } from '$lib/compose';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import HammerIcon from '~icons/ri/hammer-line';
	import DownloadIcon from '~icons/ri/download-cloud-2-line';
	import HistoryIcon from '~icons/ri/history-line';

	let { services }: { services: DeploymentService[] } = $props();
</script>

<section class="rounded-md border p-4" aria-labelledby="deploy-services-heading">
	<h3 id="deploy-services-heading" class="mb-3 text-sm font-medium">Services</h3>
	<ul class="divide-y">
		{#each services as svc (svc.service)}
			{@const badge = deploymentServiceBadge(svc)}
			{@const digest = shortDigest(svc.digest)}
			<li class="flex flex-wrap items-center justify-between gap-2 py-2 first:pt-0 last:pb-0">
				<div class="flex min-w-0 items-center gap-2">
					{#if svc.source === 'build'}
						<HammerIcon class="size-4 shrink-0 text-muted-foreground" aria-label="Built" />
					{:else if svc.source === 'reuse'}
						<HistoryIcon class="size-4 shrink-0 text-muted-foreground" aria-label="Restored" />
					{:else}
						<DownloadIcon class="size-4 shrink-0 text-muted-foreground" aria-label="Pulled" />
					{/if}
					<span class="font-mono text-sm font-medium">{svc.service}</span>
					<span
						class="hidden min-w-0 truncate font-mono text-xs text-muted-foreground sm:inline"
						title={svc.image ?? ''}
					>
						{svc.source === 'build' ? (svc.image ?? '') : (svc.source_image ?? svc.image ?? '')}
						{#if digest}<span title={svc.digest ?? ''}> @{digest}</span>{/if}
					</span>
				</div>
				<StatusBadge status={badge.status} label={badge.label} tone={badge.tone} size="sm" />
			</li>
		{/each}
	</ul>
</section>
