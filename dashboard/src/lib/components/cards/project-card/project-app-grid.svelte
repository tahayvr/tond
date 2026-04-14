<script lang="ts">
	let { appCount = 0 }: { appCount?: number } = $props();

	const GRID_SIZE = 16;
	const cells = $derived(
		Array.from({ length: GRID_SIZE }, (_, i) => ({ filled: i < appCount, i }))
	);
</script>

<div
	class="flex items-center justify-center"
	aria-label="{appCount} app{appCount === 1 ? '' : 's'}"
>
	<div class="grid grid-cols-4 gap-1">
		{#each cells as cell (cell.i)}
			<div
				class="size-4 border border-muted transition-colors {cell.filled
					? 'bg-border'
					: 'bg-transparent'}"
			></div>
		{/each}
	</div>
</div>
