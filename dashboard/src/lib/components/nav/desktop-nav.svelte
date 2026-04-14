<script lang="ts">
	import navItems from './nav-items.svelte.ts';
</script>

<nav class="flex w-full justify-between">
	<ul class="flex items-center gap-4">
		{#each navItems as item (item.href)}
			<li>
				<a
					href={item.href}
					data-sveltekit-preload-data="hover"
					class="text-xs text-muted-foreground hover:text-foreground">{item.label}</a
				>
			</li>
		{/each}
	</ul>
</nav>
