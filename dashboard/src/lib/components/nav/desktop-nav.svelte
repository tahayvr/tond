<script lang="ts">
	import { page } from '$app/state';
	import { cn } from '$lib/utils';
	import { isActive, visibleNavItems } from './nav-items.svelte.ts';

	const items = $derived(visibleNavItems());
</script>

<nav class="flex w-full justify-between" aria-label="Main">
	<ul class="flex items-center gap-4">
		{#each items as item (item.href)}
			{@const active = isActive(item.href, page.url.pathname)}
			<li>
				<a
					href={item.href}
					data-sveltekit-preload-data="hover"
					aria-current={active ? 'page' : undefined}
					class={cn(
						'text-xs text-muted-foreground hover:text-foreground',
						active && 'text-foreground underline decoration-2 underline-offset-[6px]'
					)}>{item.label}</a
				>
			</li>
		{/each}
	</ul>
</nav>
