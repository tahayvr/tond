<!--
  API description text with `inline code` rendered as <code>. Segments are
  text nodes, never HTML, so a description can't inject markup.
-->
<script lang="ts">
	import { inlineCodeSegments } from '$lib/api-reference';

	let { text }: { text: string } = $props();

	let segments = $derived(inlineCodeSegments(text));
</script>

{#each segments as segment, i (i)}{#if segment.code}<code
			class="rounded-sm bg-muted px-1 py-px font-mono text-[0.85em] text-foreground"
			>{segment.text}</code
		>{:else}{segment.text}{/if}{/each}
