<!--
  TemplateCard — one catalog entry, linking to its details and deploy page.
-->
<script lang="ts">
	import type { Template, TemplateHostMemory } from '$lib/api';
	import { categoryLabel } from '$lib/templates';
	import TemplateLogo from './template-logo.svelte';
	import MemoryBadge from './memory-badge.svelte';

	let { template, memory }: { template: Template; memory: TemplateHostMemory | null } = $props();
</script>

<a
	href="/templates/{template.id}"
	class="group flex h-full flex-col gap-3 rounded-lg border bg-card p-4 transition-colors hover:border-foreground/30 hover:bg-muted/30 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
>
	<div class="flex items-start gap-3">
		<TemplateLogo {template} />
		<div class="min-w-0 flex-1">
			<p class="truncate font-semibold group-hover:underline">{template.name}</p>
			<p class="text-xs text-muted-foreground">{categoryLabel(template.category)}</p>
		</div>
	</div>
	<p class="line-clamp-2 flex-1 text-sm text-muted-foreground">{template.description}</p>
	<MemoryBadge {template} {memory} />
</a>
