<script lang="ts">
	import { goto } from '$app/navigation';
	import { type Project } from '$lib/api';
	import {
		getUiContext,
		setUiContext,
		projectsContext,
		projectContext
	} from '$lib/ui-context.svelte';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import ProjectAppGrid from './project-app-grid.svelte';

	let { project, appCount = 0 }: { project: Project; appCount?: number } = $props();

	let ctx = $derived(getUiContext());
	let selected = $derived(ctx.type === 'project' && ctx.project.id === project.id);

	function handleSelect() {
		if (selected) {
			setUiContext(projectsContext());
		} else {
			setUiContext(projectContext(project));
		}
		goto(`/projects/${project.id}`);
	}
</script>

<button
	type="button"
	class="block h-full w-full text-left hover:cursor-pointer"
	onclick={handleSelect}
>
	<Card.Root
		class="flex h-full w-full flex-col rounded-sm border-none p-6 transition-colors hover:border  {selected
			? 'invert'
			: ''}"
	>
		<Card.Header class="p-0">
			<Card.Description class="text-xs uppercase">id: {project.id.slice(0, 8)}</Card.Description>
			<Card.Title class="text-base uppercase">{project.name}</Card.Title>
		</Card.Header>
		<Card.Content class="grow p-0">
			<div class="flex h-full flex-col justify-center py-6">
				<ProjectAppGrid {appCount} />
			</div>
		</Card.Content>
		<Card.Footer class="w-full flex-col gap-3 p-0">
			<div class="flex w-full items-center justify-between text-xs text-muted-foreground">
				<span>Updated</span>
				<div class="flex items-center gap-2">
					{#if project.active_preview_count > 0}
						<Badge variant="secondary" class="h-4 px-1.5 text-[10px]">
							{project.active_preview_count}
							{project.active_preview_count === 1 ? 'preview' : 'previews'}
						</Badge>
					{/if}
					<span>{new Date(project.updated_at).toLocaleDateString()}</span>
				</div>
			</div>
		</Card.Footer>
	</Card.Root>
</button>
