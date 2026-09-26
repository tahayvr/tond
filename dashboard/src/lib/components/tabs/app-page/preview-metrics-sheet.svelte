<script lang="ts">
	// Side panel with a preview's request and resource metrics.
	import type { PreviewDeployment } from '$lib/api';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import MetricsPanel from '$lib/components/metrics/metrics-panel.svelte';
	import RequestMetricsPanel from '$lib/components/metrics/request-metrics-panel.svelte';

	interface Props {
		preview: PreviewDeployment | null;
		onclose: () => void;
	}

	let { preview, onclose }: Props = $props();
</script>

<Sheet.Root
	open={preview !== null}
	onOpenChange={(open) => {
		if (!open) onclose();
	}}
>
	<Sheet.Content side="right" class="w-full gap-0 overflow-y-auto p-0 sm:max-w-3xl">
		{#if preview}
			<Sheet.Header class="border-b px-4 py-3 pr-10 sm:px-6">
				<Sheet.Title class="truncate">
					Preview {preview.pr_number !== null ? `#${preview.pr_number}` : preview.slug}
				</Sheet.Title>
				<Sheet.Description class="truncate font-mono text-xs">
					{preview.subdomain ?? preview.slug}
				</Sheet.Description>
			</Sheet.Header>
			<div class="space-y-8 px-4 py-4 sm:px-6">
				{#key preview.id}
					<section aria-labelledby="preview-requests-heading" class="space-y-3">
						<h3 id="preview-requests-heading" class="text-sm font-semibold">Requests</h3>
						<RequestMetricsPanel
							owner="previews"
							id={preview.id}
							noun="preview"
							initialRange="6h"
						/>
					</section>
					<section aria-labelledby="preview-resources-heading">
						<h3 id="preview-resources-heading" class="text-sm font-semibold">Resources</h3>
						<MetricsPanel owner="previews" id={preview.id} noun="preview" />
					</section>
				{/key}
			</div>
		{/if}
	</Sheet.Content>
</Sheet.Root>
