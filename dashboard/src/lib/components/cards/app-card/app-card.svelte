<script lang="ts">
	import { goto } from '$app/navigation';
	import { type App, type Project } from '$lib/api';
	import { getUiContext, setUiContext, appContext, projectsContext } from '$lib/ui-context.svelte';
	import * as Card from '$lib/components/ui/card/index.js';
	import ExternalLink from '@lucide/svelte/icons/external-link';

	interface Props {
		app: App;
		project?: Project;
	}

	let { app, project }: Props = $props();

	let ctx = $derived(getUiContext());
	let selected = $derived(ctx.type === 'app' && ctx.app.id === app.id);

	function statusDotClass(status: string): string {
		switch (status) {
			case 'running':
				return 'bg-accent';
			case 'failed':
				return 'bg-destructive';
			case 'unhealthy':
				return 'bg-warn';
			default:
				return 'bg-muted-foreground/40';
		}
	}

	function handleSelect() {
		if (selected) {
			setUiContext(projectsContext());
		} else {
			setUiContext(appContext(app));
		}
		goto(`/apps/${app.id}`);
	}
</script>

<div class="h-full">
	<button
		type="button"
		class="block h-full w-full text-left hover:cursor-pointer"
		onclick={handleSelect}
	>
		<Card.Root
			class="flex h-full w-full flex-col rounded-sm border-none p-6 transition-colors hover:border {selected
				? 'invert'
				: ''}"
		>
			<Card.Header class="p-0">
				<div class="flex items-start justify-between gap-2">
					<Card.Description class="text-xs uppercase">id: {app.id.slice(0, 8)}</Card.Description>
					{#if project}
						<span class="font-mono text-[10px] text-muted-foreground/50 uppercase"
							>{project.name}</span
						>
					{/if}
				</div>
				<Card.Title class="text-base uppercase">{app.name}</Card.Title>
				<div class="flex items-center gap-2">
					<span class="inline-block size-2 shrink-0 rounded-full {statusDotClass(app.status)}"
					></span>
					<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
						>{app.status}</span
					>
				</div>
			</Card.Header>
			<Card.Content class="grow p-0">
				<div class="flex h-full flex-col justify-center gap-3 py-6">
					<!-- Source -->
					<p class="text-center font-mono text-[10px] text-muted-foreground/70 uppercase">
						{app.source_type}{app.source_url ? ` · ${app.source_url.split('/').at(-1)}` : ''}
					</p>

					<!-- Domain -->
					{#if app.domain}
						<div class="flex items-center justify-center gap-1">
							<ExternalLink class="h-3 w-3 shrink-0 text-muted-foreground/60" />
							<span class="truncate font-mono text-[10px] text-muted-foreground/70">
								{app.domain}
							</span>
						</div>
					{/if}
				</div>
			</Card.Content>
			<Card.Footer class="w-full p-0"></Card.Footer>
		</Card.Root>
	</button>
</div>
