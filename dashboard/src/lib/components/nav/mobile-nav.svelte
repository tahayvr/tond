<script lang="ts">
	import { page } from '$app/state';
	import MenuIcon from '~icons/ri/menu-line';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { cn } from '$lib/utils';
	import navItems, { isActive } from './nav-items.svelte.ts';

	let { email, onLogout }: { email?: string; onLogout: () => void } = $props();

	let open = $state(false);

	// Close the menu after navigating.
	$effect(() => {
		void page.url.pathname;
		open = false;
	});
</script>

<Sheet.Root bind:open>
	<Sheet.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="ghost" size="icon" aria-label="Open menu">
				<MenuIcon class="size-5" />
			</Button>
		{/snippet}
	</Sheet.Trigger>
	<Sheet.Content side="left" class="w-72 gap-0 p-0">
		<Sheet.Header class="border-b px-4 py-3">
			<Sheet.Title class="text-sm">Menu</Sheet.Title>
			{#if email}
				<Sheet.Description class="truncate text-xs lowercase">{email}</Sheet.Description>
			{/if}
		</Sheet.Header>
		<nav class="flex-1 overflow-y-auto p-2" aria-label="Main">
			<ul class="flex flex-col gap-1">
				{#each navItems as item (item.href)}
					{@const active = isActive(item.href, page.url.pathname)}
					<li>
						<a
							href={item.href}
							data-sveltekit-preload-data="tap"
							aria-current={active ? 'page' : undefined}
							class={cn(
								'block rounded px-3 py-2.5 text-sm text-muted-foreground hover:bg-muted hover:text-foreground',
								active && 'bg-muted text-foreground'
							)}>{item.label}</a
						>
					</li>
				{/each}
			</ul>
		</nav>
		<div class="flex flex-col gap-1 border-t p-2">
			<a
				href="/profile"
				class="block rounded px-3 py-2.5 text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
				>Profile</a
			>
			<button
				type="button"
				onclick={onLogout}
				class="block rounded px-3 py-2.5 text-left text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
				>Logout</button
			>
		</div>
	</Sheet.Content>
</Sheet.Root>
