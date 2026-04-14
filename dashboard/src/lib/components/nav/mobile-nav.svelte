<script lang="ts">
	import navItems from './nav-items.svelte.ts';
</script>

<nav class="p-2">
	<ul class="flex flex-col gap-2">
		{#each navItems as item (item.href)}
			<li>
				<a
					href={item.href}
					data-sveltekit-preload-data="tap"
					class="block rounded px-3 py-2 text-xs text-muted-foreground hover:bg-muted"
					>{item.label}</a
				>
			</li>
		{/each}
	</ul>
</nav>
