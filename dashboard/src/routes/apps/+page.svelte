<script lang="ts">
	import { can } from '$lib/auth.svelte';
	import type { App, Project } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import Plus from '~icons/ri/add-line';
	import Box from '~icons/ri/box-3-line';
	import LayoutTemplate from '~icons/ri/layout-masonry-line';
	import AppGrid from '$lib/components/cards/app-card/app-grid.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let apps = $derived(data.apps as App[]);
	let projects = $derived(data.projects as Project[]);
	let searchQuery = $state('');

	let runningCount = $derived(apps.filter((a) => a.status === 'running').length);

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="Apps"
		description={apps.length ? `${runningCount} of ${apps.length} running` : undefined}
	>
		{#snippet actions()}
			{#if can('apps:write')}
				<Button variant="outline" href="/templates">
					<LayoutTemplate class="mr-1 h-4 w-4" />
					From a template
				</Button>
				<Button href="/apps/new">
					<Plus class="mr-1 h-4 w-4" />
					New app
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	{#if apps.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Box class="mb-4 h-12 w-12 text-muted-foreground" />
			<p class="font-medium">No apps yet</p>
			<p class="mt-2 max-w-md text-center text-sm text-muted-foreground">
				Deploy from a Git repository, a Docker image or a Compose file, or start from a ready-made
				template like Umami, n8n or Vaultwarden.
			</p>
			<div class="mt-4 flex flex-wrap justify-center gap-2">
				<Button href="/apps/new">
					<Plus class="mr-1 h-4 w-4" />
					Create app
				</Button>
				<Button variant="outline" href="/templates">
					<LayoutTemplate class="mr-1 h-4 w-4" />
					Browse templates
				</Button>
			</div>
		</div>
	{:else}
		<AppGrid {apps} {projects} bind:searchQuery />
	{/if}
</div>
