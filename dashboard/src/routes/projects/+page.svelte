<script lang="ts">
	import type { Project, App } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import ProjectCard from '$lib/components/cards/project-card/project-card.svelte';
	import CreateProjectDialog from '$lib/components/dialogs/create-project-dialog.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Search from '~icons/ri/search-line';
	import Plus from '~icons/ri/add-line';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let projects = $derived(data.projects as Project[]);
	let apps = $derived(data.apps as App[]);
	let showCreate = $state(false);
	let searchQuery = $state('');

	let appCountByProject = $derived(
		apps.reduce<Record<string, number>>((acc, app) => {
			if (app.project_id) acc[app.project_id] = (acc[app.project_id] ?? 0) + 1;
			return acc;
		}, {})
	);

	let filteredProjects = $derived(
		projects.filter(
			(p) =>
				p.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				(p.description ?? '').toLowerCase().includes(searchQuery.toLowerCase())
		)
	);

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader title="Projects">
		{#snippet actions()}
			<Button variant="outline" size="sm" href="/apps/new">
				<Plus class="mr-1 h-4 w-4" />
				New app
			</Button>
			<Button size="sm" onclick={() => (showCreate = true)}>
				<Plus class="mr-1 h-4 w-4" />
				New project
			</Button>
		{/snippet}
	</PageHeader>

	<!-- Search -->
	{#if projects.length > 0}
		<div class="relative">
			<Search class="absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
			<Input
				type="search"
				aria-label="Search projects"
				placeholder="Search projects…"
				bind:value={searchQuery}
				class="pl-9"
			/>
		</div>
	{/if}

	{#if projects.length === 0}
		<div class="rounded-lg border border-dashed py-16 text-center">
			<p class="text-lg font-medium text-muted-foreground">No projects yet</p>
			<p class="mt-2 text-sm text-muted-foreground">Create your first project to get started</p>
			<Button class="mt-4" onclick={() => (showCreate = true)}>
				<Plus class="mr-1 h-4 w-4" />
				Create project
			</Button>
		</div>
	{:else if filteredProjects.length === 0}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No projects match "{searchQuery}"</p>
		</div>
	{:else}
		<div class="grid grid-cols-1 items-stretch gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
			{#each filteredProjects as project (project.id)}
				<ProjectCard {project} appCount={appCountByProject[project.id] ?? 0} />
			{/each}
		</div>
	{/if}
</div>

<CreateProjectDialog bind:open={showCreate} />
