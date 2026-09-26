<!--
  MemoryBadge — a template's memory estimate, "Fits on 2 GB" when it does, and
  a warning tint when it needs more than this server has free.

  <MemoryBadge template={t} memory={catalog.memory} />
-->
<script lang="ts">
	import type { Template, TemplateHostMemory } from '$lib/api';
	import { formatRam, memoryFit } from '$lib/templates';
	import { cn } from '$lib/utils';
	import MemoryIcon from '~icons/ri/cpu-line';
	import CheckIcon from '~icons/ri/check-line';
	import AlertIcon from '~icons/ri/alert-line';

	let {
		template,
		memory
	}: {
		template: Pick<Template, 'ram_mb' | 'fits_2gb'>;
		memory: TemplateHostMemory | null;
	} = $props();

	let fit = $derived(memoryFit(template, memory));
</script>

<span class="inline-flex flex-wrap items-center gap-1.5 text-xs">
	<span
		class={cn(
			'inline-flex items-center gap-1 rounded-sm border px-1.5 py-0.5 tabular-nums',
			fit === 'short'
				? 'border-destructive/40 bg-destructive/10 text-destructive'
				: fit === 'tight'
					? 'border-warn/40 bg-warn/10 text-warn'
					: 'text-muted-foreground'
		)}
		title={fit === 'short'
			? 'Needs more memory than this server has free'
			: fit === 'tight'
				? 'Uses most of the free memory'
				: 'Typical memory use'}
	>
		{#if fit === 'short' || fit === 'tight'}
			<AlertIcon class="size-3" aria-hidden="true" />
		{:else}
			<MemoryIcon class="size-3" aria-hidden="true" />
		{/if}
		~{formatRam(template.ram_mb)}
		{#if fit === 'short'}<span class="sr-only">, more than is free</span>{/if}
	</span>
	{#if template.fits_2gb}
		<span
			class="inline-flex items-center gap-1 rounded-sm border border-success/40 bg-success/10 px-1.5 py-0.5 text-success"
		>
			<CheckIcon class="size-3" aria-hidden="true" />
			Fits on 2 GB
		</span>
	{/if}
</span>
