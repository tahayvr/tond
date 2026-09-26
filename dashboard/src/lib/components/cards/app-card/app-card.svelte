<script lang="ts">
	import { type App, type Project } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import ExternalLink from '~icons/ri/external-link-line';

	interface Props {
		app: App;
		project?: Project;
	}

	let { app, project }: Props = $props();
</script>

<a href={`/apps/${app.id}`} class="block h-full rounded-sm" data-sveltekit-preload-data="hover">
	<Card.Root
		class="flex h-full w-full flex-col gap-3 rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="gap-2 p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Title class="min-w-0 truncate text-base">{app.name}</Card.Title>
				<StatusBadge status={app.status} kind="app" size="sm" />
			</div>
			{#if project}
				<Card.Description class="truncate text-xs">{project.name}</Card.Description>
			{/if}
		</Card.Header>
		<Card.Content class="grid grow content-end gap-1.5 p-0 text-xs text-muted-foreground">
			<p class="truncate font-mono">
				{app.source_type}{app.source_url ? ` · ${app.source_url.split('/').at(-1)}` : ''}
			</p>
			{#if app.domain}
				<p class="flex min-w-0 items-center gap-1">
					<ExternalLink aria-hidden="true" class="h-3 w-3 shrink-0" />
					<span class="truncate font-mono">{app.domain}</span>
				</p>
			{/if}
		</Card.Content>
	</Card.Root>
</a>
