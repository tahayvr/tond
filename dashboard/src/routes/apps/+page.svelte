<script lang="ts">
	import type { App, Project } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { goto } from '$app/navigation';
	import Plus from '~icons/ri/add-line';
	import Box from '~icons/ri/box-3-line';
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
	<!-- Header -->
	<div class="flex h-14 items-center justify-between gap-4">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Applications</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{runningCount}/{apps.length} UNITS OPERATIONAL
			</p>
		</div>
		<Button onclick={() => goto('/apps/new')}>
			<Plus class="mr-1 h-4 w-4" />
			New App
		</Button>
	</div>

	{#if apps.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Box class="mb-4 h-12 w-12 text-muted-foreground/40" />
			<p class="font-mono text-[11px] tracking-widest text-muted-foreground/60 uppercase">
				NO_UNITS_FOUND
			</p>
			<p class="mt-2 text-sm text-muted-foreground/70">Create your first application</p>
			<Button class="mt-4" onclick={() => goto('/apps/new')}>
				<Plus class="mr-1 h-4 w-4" />
				Create App
			</Button>
		</div>
	{:else}
		<AppGrid {apps} {projects} bind:searchQuery />
	{/if}
</div>
