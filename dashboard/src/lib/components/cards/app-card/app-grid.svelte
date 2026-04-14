<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import Search from '@lucide/svelte/icons/search';
	import Rocket from '@lucide/svelte/icons/rocket';
	import AppCard from './app-card.svelte';
	import type { App, Project } from '$lib/api';

	interface Props {
		apps: App[];
		projects: Project[];
		searchQuery?: string;
	}

	let { apps, projects, searchQuery = $bindable('') }: Props = $props();

	let projectById = $derived(
		projects.reduce<Record<string, Project>>((acc, p) => {
			acc[p.id] = p;
			return acc;
		}, {})
	);

	let filteredApps = $derived(
		apps
			.filter(
				(app) =>
					app.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
					(app.domain ?? '').toLowerCase().includes(searchQuery.toLowerCase()) ||
					(app.source_url ?? '').toLowerCase().includes(searchQuery.toLowerCase())
			)
			.toSorted((a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime())
	);
</script>

{#if apps.length > 0 || searchQuery}
	<div class="relative">
		<Search class="absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-muted-foreground/70" />
		<Input
			type="text"
			placeholder="Search apps..."
			bind:value={searchQuery}
			class="border-border bg-card pl-9 font-mono text-sm"
		/>
	</div>
{/if}

{#if filteredApps.length === 0 && apps.length > 0}
	<div class="rounded-lg border border-border py-12 text-center">
		<p class="font-mono text-[11px] tracking-wider text-muted-foreground/70 uppercase">
			NO_MATCHING_UNITS
		</p>
	</div>
{:else if filteredApps.length > 0}
	<div class="relative overflow-hidden">
		<div
			class="pointer-events-none absolute inset-0 opacity-30"
			style="background-image: 
				linear-gradient(to right, rgba(255,255,255,0.03) 1px, transparent 1px),
				linear-gradient(to bottom, rgba(255,255,255,0.03) 1px, transparent 1px);
			background-size: 40px 40px;"
		></div>

		<div class="relative">
			<div class="mb-4 flex items-center gap-2">
				<Rocket class="h-4 w-4 text-muted-foreground/70" />
				<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
					DEPLOYED_UNITS ({filteredApps.length})
				</span>
			</div>

			<div
				class="grid grid-cols-1 items-stretch gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
			>
				{#each filteredApps as app (app.id)}
					<AppCard {app} project={app.project_id ? projectById[app.project_id] : undefined} />
				{/each}
			</div>
		</div>
	</div>
{/if}
