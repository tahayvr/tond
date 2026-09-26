<script lang="ts">
	import type { TemplateCatalog } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { categoryCounts, filterTemplates, formatRam } from '$lib/templates';
	import { cn } from '$lib/utils';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import TemplateCard from '$lib/components/templates/template-card.svelte';
	import SearchIcon from '~icons/ri/search-line';
	import MemoryIcon from '~icons/ri/cpu-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let catalog = $derived(data.catalog as TemplateCatalog);
	let query = $state('');
	// Starts from ?category= (a link from a template's page); the chips change it.
	let category = $derived(data.category as string);
	let fitsOnly = $state(false);

	let categories = $derived(categoryCounts(catalog.templates));
	let shown = $derived(filterTemplates(catalog.templates, { query, category, fitsOnly }));

	function handleCategory(id: string) {
		category = category === id ? '' : id;
		const url = new URL(window.location.href);
		if (category) url.searchParams.set('category', category);
		else url.searchParams.delete('category');
		history.replaceState(history.state, '', url);
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="Templates"
		description="Ready-made apps with sensible defaults. Pick one, fill in a name, and Tond deploys it with generated passwords and HTTPS."
	>
		{#snippet meta()}
			{#if catalog.memory}
				<span class="inline-flex items-center gap-1.5">
					<MemoryIcon class="size-4 shrink-0" aria-hidden="true" />
					<span>
						This server has
						<strong class="font-semibold text-foreground"
							>{formatRam(catalog.memory.available_mb)}</strong
						>
						of {formatRam(catalog.memory.total_mb)} memory free
					</span>
				</span>
			{/if}
		{/snippet}
	</PageHeader>

	<div class="flex flex-col gap-3 sm:flex-row sm:items-center">
		<div class="relative flex-1">
			<SearchIcon
				class="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground"
				aria-hidden="true"
			/>
			<Input
				type="search"
				placeholder="Search: analytics, git, wiki…"
				aria-label="Search templates"
				bind:value={query}
				class="pl-9"
			/>
		</div>
		<div class="flex items-center gap-2">
			<Checkbox id="fits-only" bind:checked={fitsOnly} />
			<Label for="fits-only" class="text-sm font-normal whitespace-nowrap"
				>Only ones that fit on 2 GB</Label
			>
		</div>
	</div>

	<nav aria-label="Categories" class="-mx-4 overflow-x-auto px-4 sm:mx-0 sm:px-0">
		<ul class="flex gap-2 sm:flex-wrap">
			<li>
				<button
					type="button"
					aria-pressed={category === ''}
					onclick={() => handleCategory('')}
					class={cn(
						'inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm whitespace-nowrap transition-colors',
						category === ''
							? 'border-foreground bg-foreground text-background'
							: 'text-muted-foreground hover:bg-muted hover:text-foreground'
					)}
				>
					All
					<span
						class={cn(
							'rounded-sm px-1.5 text-xs tabular-nums',
							category === '' ? 'bg-background/20' : 'bg-muted'
						)}>{catalog.templates.length}</span
					>
				</button>
			</li>
			{#each categories as c (c.id)}
				<li>
					<button
						type="button"
						aria-pressed={category === c.id}
						onclick={() => handleCategory(c.id)}
						class={cn(
							'inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm whitespace-nowrap transition-colors',
							category === c.id
								? 'border-foreground bg-foreground text-background'
								: 'text-muted-foreground hover:bg-muted hover:text-foreground'
						)}
					>
						{c.label}
						<span
							class={cn(
								'rounded-sm px-1.5 text-xs tabular-nums',
								category === c.id ? 'bg-background/20' : 'bg-muted'
							)}>{c.count}</span
						>
					</button>
				</li>
			{/each}
		</ul>
	</nav>

	{#if shown.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-dashed px-4 py-16 text-center"
		>
			<SearchIcon class="mb-3 size-10 text-muted-foreground" aria-hidden="true" />
			<p class="font-medium">No templates match</p>
			<p class="mt-1 max-w-md text-sm text-muted-foreground">
				Try another word or category. You can always deploy any image or Compose file from
				<a href="/apps/new" class="underline">New app</a>.
			</p>
		</div>
	{:else}
		<ul class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4" aria-label="Templates">
			{#each shown as template (template.id)}
				<li>
					<TemplateCard {template} memory={catalog.memory} serverArch={catalog.architecture} />
				</li>
			{/each}
		</ul>
		<p class="text-xs text-muted-foreground">
			Memory figures are typical use with light traffic. "Fits on 2 GB" means the app runs
			comfortably on a 2 GB server next to Tond.
		</p>
	{/if}
</div>
