<script lang="ts">
	// Line / swatch legend for a TimeChart (colours are theme variables).
	interface Props {
		items: { label: string; color: string; dashed?: boolean }[];
	}

	let { items }: Props = $props();
</script>

<ul class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
	{#each items as item (item.label)}
		<li class="flex items-center gap-1.5">
			<svg width="16" height="4" aria-hidden="true" class="shrink-0">
				<line
					x1="0"
					x2="16"
					y1="2"
					y2="2"
					stroke={item.color}
					stroke-width="3"
					stroke-dasharray={item.dashed ? '4 2' : undefined}
				/>
			</svg>
			{item.label}
		</li>
	{/each}
</ul>
