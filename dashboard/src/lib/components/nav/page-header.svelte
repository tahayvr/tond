<!--
  PageHeader — the page's <h1>, optional status/meta line and actions, plus the
  document title. Stacks on phones, one row on wider screens.

  <PageHeader title={app.name} section="Logs">
    {#snippet meta()}<StatusBadge status={app.status} kind="app" />{/snippet}
    {#snippet actions()}<Button>Deploy</Button>{/snippet}
  </PageHeader>

  Titles render exactly as given: user-provided names are never re-cased.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import PageTitle from './page-title.svelte';

	let {
		title,
		description,
		section,
		docTitle,
		meta,
		actions
	}: {
		title: string;
		description?: string;
		/** Extra, more specific part for the document title (e.g. the active tab). */
		section?: string | null;
		/** Document title when it should differ from the heading. */
		docTitle?: string;
		meta?: Snippet;
		actions?: Snippet;
	} = $props();
</script>

<PageTitle title={docTitle ?? title} {section} />

<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
	<div class="min-w-0 space-y-1.5">
		<h1 class="text-2xl font-extrabold break-words sm:text-3xl">{title}</h1>
		{#if description}
			<p class="text-sm text-muted-foreground">{description}</p>
		{/if}
		{#if meta}
			<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
				{@render meta()}
			</div>
		{/if}
	</div>
	{#if actions}
		<div class="flex shrink-0 flex-wrap items-center gap-2">
			{@render actions()}
		</div>
	{/if}
</div>
