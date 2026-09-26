<script lang="ts">
	import { type Project } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import ProjectAppGrid from './project-app-grid.svelte';

	let { project, appCount = 0 }: { project: Project; appCount?: number } = $props();
</script>

<a
	href={`/projects/${project.id}`}
	class="block h-full rounded-sm"
	data-sveltekit-preload-data="hover"
>
	<Card.Root
		class="flex h-full w-full flex-col rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="p-0">
			<Card.Title class="truncate text-base">{project.name}</Card.Title>
			{#if project.description}
				<Card.Description class="line-clamp-2 text-xs">{project.description}</Card.Description>
			{/if}
		</Card.Header>
		<Card.Content class="grow p-0">
			<div class="flex h-full flex-col justify-center py-4">
				<ProjectAppGrid {appCount} />
			</div>
		</Card.Content>
		<Card.Footer class="w-full flex-col gap-3 p-0">
			<div class="flex w-full items-center justify-between text-xs text-muted-foreground">
				<span>Updated</span>
				<div class="flex items-center gap-2">
					{#if project.active_preview_count > 0}
						<Badge variant="secondary" class="h-5 px-1.5 text-xs">
							{project.active_preview_count}
							{project.active_preview_count === 1 ? 'preview' : 'previews'}
						</Badge>
					{/if}
					<span>{new Date(project.updated_at).toLocaleDateString()}</span>
				</div>
			</div>
		</Card.Footer>
	</Card.Root>
</a>
