<script lang="ts">
	import type { Project, App } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import ProjectCard from '$lib/components/cards/project-card/project-card.svelte';
	import CreateProjectDialog from '$lib/components/dialogs/create-project-dialog.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import Search from '@lucide/svelte/icons/search';
	import Plus from '@lucide/svelte/icons/plus';
	import { goto } from '$app/navigation';
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
	<div class="flex h-14 items-center justify-between gap-4">
		<h2 class="text-3xl font-extrabold uppercase">Projects</h2>
		<div class="flex items-center gap-2">
			<Button variant="outline" size="sm" onclick={() => (showCreate = true)}>
				<Plus class="mr-1 h-4 w-4" />
				New Project
			</Button>
			<Button variant="outline" size="sm" onclick={() => goto('/apps/new')}>
				<Plus class="mr-1 h-4 w-4" />
				New App
			</Button>
		</div>
	</div>

	<!-- Search -->
	{#if projects.length > 0}
		<div class="relative">
			<Search class="absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
			<Input type="text" placeholder="Search projects..." bind:value={searchQuery} class="pl-9" />
		</div>
	{/if}

	{#if projects.length === 0}
		<div class="rounded-lg border border-dashed py-16 text-center">
			<p class="text-lg font-medium text-muted-foreground">No projects yet</p>
			<p class="mt-2 text-sm text-muted-foreground/60">Create your first project to get started</p>
			<Button class="mt-4" onclick={() => (showCreate = true)}>
				<Plus class="mr-1 h-4 w-4" />
				Create Project
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
