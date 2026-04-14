<script lang="ts">
	import { getUiContext } from '$lib/ui-context.svelte';
	import Waypoints from '@lucide/svelte/icons/waypoints';

	let ctx = $derived(getUiContext());
	let feed = $derived(ctx.feed);
</script>

<div class="flex items-center justify-between gap-2">
	<div class="flex items-center gap-1.5">
		<Waypoints size="14" />
		<span class="text-xs font-semibold uppercase group-data-[collapsible=icon]:hidden">
			{feed.title}
		</span>
	</div>
	{#if feed.label}
		<div class="group-data-[collapsible=icon]:hidden">
			<span class="text-xs text-muted-foreground">{feed.label}</span>
		</div>
	{/if}
</div>
