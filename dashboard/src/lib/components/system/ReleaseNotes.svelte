<!--
  ReleaseNotes — a release's notes (Markdown from GitHub) rendered as plain
  elements through parseReleaseNotes(): no HTML from the notes reaches the page.
-->
<script lang="ts">
	import { parseReleaseNotes, type Inline } from '$lib/updates';

	let { markdown }: { markdown: string } = $props();

	let blocks = $derived(parseReleaseNotes(markdown));
</script>

{#snippet inline(content: Inline[])}
	{#each content as part, i (i)}
		{#if part.type === 'code'}
			<code class="rounded-sm bg-muted px-1 py-0.5 text-[0.85em]">{part.text}</code>
		{:else if part.type === 'strong'}
			<strong class="font-semibold">{part.text}</strong>
		{:else if part.type === 'link'}
			<a
				href={part.href}
				target="_blank"
				rel="noopener noreferrer"
				class="text-info underline underline-offset-2 hover:no-underline">{part.text}</a
			>
		{:else}{part.text}{/if}
	{/each}
{/snippet}

<div class="space-y-2 text-sm leading-relaxed">
	{#if blocks.length === 0}
		<p class="text-muted-foreground">No release notes.</p>
	{/if}
	{#each blocks as block, i (i)}
		{#if block.type === 'heading'}
			<p class="pt-1 font-semibold">{@render inline(block.content)}</p>
		{:else if block.type === 'paragraph'}
			<p>{@render inline(block.content)}</p>
		{:else if block.type === 'list'}
			{#if block.ordered}
				<ol class="list-decimal space-y-1 pl-5">
					{#each block.items as item, j (j)}
						<li>{@render inline(item)}</li>
					{/each}
				</ol>
			{:else}
				<ul class="list-disc space-y-1 pl-5">
					{#each block.items as item, j (j)}
						<li>{@render inline(item)}</li>
					{/each}
				</ul>
			{/if}
		{:else}
			<pre class="overflow-x-auto rounded-sm bg-muted p-2 text-xs"><code>{block.text}</code></pre>
		{/if}
	{/each}
</div>
