<script lang="ts">
	import { getUiContext } from '$lib/ui-context.svelte';
	import Waypoints from '~icons/ri/route-line';

	let ctx = $derived(getUiContext());
	let feed = $derived(ctx.feed);
</script>

<div class="flex items-center justify-between gap-2">
	<div class="flex items-center gap-1.5">
		<Waypoints class="size-3.5" />
		<span class="text-xs font-semibold group-data-[collapsible=icon]:hidden">
			{feed.title}
		</span>
	</div>
	{#if feed.label}
		<div class="group-data-[collapsible=icon]:hidden">
			<span class="text-xs text-muted-foreground">{feed.label}</span>
		</div>
	{/if}
</div>
