<!--
  TemplateLogo — a template's logo on a calm tile, or its monogram.

  <TemplateLogo template={t} size="lg" />
-->
<script lang="ts">
	import type { Template } from '$lib/api';
	import { cn } from '$lib/utils';
	import { TEMPLATE_LOGOS, monogram } from './template-logos';

	let {
		template,
		size = 'md',
		class: className
	}: {
		template: Pick<Template, 'name' | 'logo'>;
		size?: 'md' | 'lg';
		class?: string;
	} = $props();

	let Logo = $derived(template.logo ? TEMPLATE_LOGOS[template.logo] : undefined);
</script>

<div
	aria-hidden="true"
	class={cn(
		'flex shrink-0 items-center justify-center rounded-md border bg-muted/60 text-foreground',
		size === 'lg' ? 'size-14' : 'size-10',
		className
	)}
>
	{#if Logo}
		<Logo class={size === 'lg' ? 'size-7' : 'size-5'} />
	{:else}
		<span class={cn('font-bold tracking-tight', size === 'lg' ? 'text-lg' : 'text-sm')}
			>{monogram(template.name)}</span
		>
	{/if}
</div>
