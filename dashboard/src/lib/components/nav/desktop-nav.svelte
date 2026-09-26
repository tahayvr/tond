<script lang="ts">
	import { page } from '$app/state';
	import ArrowDown from '~icons/ri/arrow-down-s-line';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import { cn } from '$lib/utils';
	import { isActive, visibleNavItems } from './nav-items.svelte.ts';

	const items = $derived(visibleNavItems());
	const primary = $derived(items.filter((item) => item.primary));
	const more = $derived(items.filter((item) => !item.primary));
	const moreActive = $derived(more.some((item) => isActive(item.href, page.url.pathname)));

	let moreOpen = $state(false);

	// Close the menu after navigating.
	$effect(() => {
		void page.url.pathname;
		moreOpen = false;
	});

	const linkClass = (active: boolean) =>
		cn(
			'text-xs whitespace-nowrap text-muted-foreground hover:text-foreground',
			active && 'text-foreground underline decoration-2 underline-offset-[6px]'
		);
</script>

<nav class="flex w-full justify-between" aria-label="Main">
	<ul class="flex items-center gap-4">
		{#each primary as item (item.href)}
			{@const active = isActive(item.href, page.url.pathname)}
			<li>
				<a
					href={item.href}
					data-sveltekit-preload-data="hover"
					aria-current={active ? 'page' : undefined}
					class={linkClass(active)}>{item.label}</a
				>
			</li>
		{/each}
		{#if more.length > 0}
			<li>
				<Popover.Root bind:open={moreOpen}>
					<Popover.Trigger
						class={cn(linkClass(moreActive), 'inline-flex items-center gap-0.5')}
						aria-label="More pages"
					>
						More <ArrowDown class="size-3.5" />
					</Popover.Trigger>
					<Popover.Content class="w-48 p-1" align="start">
						<ul class="flex flex-col">
							{#each more as item (item.href)}
								{@const active = isActive(item.href, page.url.pathname)}
								<li>
									<a
										href={item.href}
										data-sveltekit-preload-data="hover"
										aria-current={active ? 'page' : undefined}
										class={cn(
											'block rounded-sm px-2 py-1.5 text-sm text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:bg-accent focus-visible:outline-none',
											active && 'bg-accent text-foreground'
										)}>{item.label}</a
									>
								</li>
							{/each}
						</ul>
					</Popover.Content>
				</Popover.Root>
			</li>
		{/if}
	</ul>
</nav>
