<!--
  LinkTabs — tab navigation made of real links, so the active tab is in the URL.

  <LinkTabs label="App sections" tabs={APP_TABS} active={tab}
    href={(id) => appTabHref(app.id, id)} />

  `variant="pills"` renders a vertical list on wide screens (used for sub-sections).
-->
<script lang="ts" generics="T extends string">
	import type { TabDef } from '$lib/tabs';
	import { cn } from '$lib/utils';

	let {
		tabs,
		active,
		href,
		label,
		variant = 'underline',
		class: className
	}: {
		tabs: readonly TabDef<T>[];
		active: T;
		href: (id: T) => string;
		/** Accessible name for the <nav>. */
		label: string;
		variant?: 'underline' | 'pills';
		class?: string;
	} = $props();

	let list = $state<HTMLUListElement | null>(null);

	// Keep the active tab visible when the row scrolls horizontally (phones).
	$effect(() => {
		void active;
		const el = list?.querySelector<HTMLElement>('[aria-current="page"]');
		if (!list || !el || list.scrollWidth <= list.clientWidth) return;
		const left = el.offsetLeft - list.offsetLeft;
		if (left < list.scrollLeft || left + el.offsetWidth > list.scrollLeft + list.clientWidth) {
			list.scrollLeft = left - 16;
		}
	});
</script>

<nav aria-label={label} class={cn('min-w-0', className)}>
	<ul
		bind:this={list}
		class={cn(
			'flex overflow-x-auto [scrollbar-width:none]',
			variant === 'underline' && 'gap-1 border-b',
			variant === 'pills' && 'gap-1 md:flex-col md:overflow-visible'
		)}
	>
		{#each tabs as tab (tab.id)}
			{@const current = tab.id === active}
			<li class="shrink-0">
				<a
					href={href(tab.id)}
					data-sveltekit-noscroll
					aria-current={current ? 'page' : undefined}
					class={cn(
						'block rounded-sm text-sm whitespace-nowrap transition-colors',
						variant === 'underline' &&
							'-mb-px border-b-2 px-2.5 py-2 text-muted-foreground hover:text-foreground sm:px-3',
						variant === 'underline' && current && 'border-foreground font-medium text-foreground',
						variant === 'underline' && !current && 'border-transparent',
						variant === 'pills' && 'px-3 py-1.5 text-muted-foreground hover:bg-muted hover:text-foreground',
						variant === 'pills' && current && 'bg-muted font-medium text-foreground'
					)}>{tab.label}</a
				>
			</li>
		{/each}
	</ul>
</nav>
