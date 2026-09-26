<!--
  AppEventsList — the app's Events tab: every event, grouped by day, with a
  category filter and "Load older".

  <AppEventsList appId={app.id} />
-->
<script lang="ts">
	import { getAppEvents, type AppEvent } from '$lib/api';
	import { EVENT_CATEGORIES, filterEvents, groupByDay, type EventCategory } from '$lib/app-events';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { cn } from '$lib/utils';
	import AppEventRow from './app-event-row.svelte';
	import PulseIcon from '~icons/ri/pulse-line';

	let { appId }: { appId: string } = $props();

	const PAGE = 50;

	let events = $state<AppEvent[] | null>(null);
	let nextBefore = $state<string | null>(null);
	let category = $state<EventCategory | 'all'>('all');
	let loadingMore = $state(false);
	let error = $state('');
	let now = $state(Date.now());

	let shown = $derived(events ? filterEvents(events, category) : []);
	let groups = $derived(groupByDay(shown, now));

	async function load(id: string) {
		try {
			const page = await getAppEvents(id, { limit: PAGE });
			if (id !== appId) return;
			events = page.events;
			nextBefore = page.next_before;
			error = '';
		} catch (e: any) {
			error = e.message;
		} finally {
			now = Date.now();
		}
	}

	async function handleLoadMore() {
		if (!nextBefore) return;
		loadingMore = true;
		error = '';
		try {
			const page = await getAppEvents(appId, { limit: PAGE, before: nextBefore });
			events = [...(events ?? []), ...page.events];
			nextBefore = page.next_before;
		} catch (e: any) {
			error = e.message;
		} finally {
			loadingMore = false;
		}
	}

	$effect(() => {
		const id = appId;
		events = null;
		load(id);
	});
</script>

<div class="space-y-4 pt-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<p class="text-sm text-muted-foreground">
			Deploys, restarts, crashes, out-of-memory kills, certificates and configuration changes,
			newest first.
		</p>
		<div class="flex flex-wrap gap-1.5" role="group" aria-label="Filter events">
			{#each EVENT_CATEGORIES as c (c.id)}
				<button
					type="button"
					aria-pressed={category === c.id}
					onclick={() => (category = c.id)}
					class={cn(
						'rounded-md border px-2.5 py-1 text-xs transition-colors',
						category === c.id
							? 'border-foreground bg-foreground text-background'
							: 'text-muted-foreground hover:bg-muted hover:text-foreground'
					)}>{c.label}</button
				>
			{/each}
		</div>
	</div>

	<InlineError message={error} ondismiss={() => (error = '')} />

	{#if events === null && !error}
		<div class="space-y-2">
			{#each Array(5) as _, i (i)}
				<Skeleton class="h-10" />
			{/each}
		</div>
	{:else if events && shown.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-dashed px-4 py-12 text-center"
		>
			<PulseIcon class="mb-3 size-8 text-muted-foreground" aria-hidden="true" />
			<p class="font-medium">
				{events.length === 0 ? 'No events yet' : 'Nothing in this category'}
			</p>
			<p class="mt-1 text-sm text-muted-foreground">
				{events.length === 0
					? 'Deploy the app and its history starts here.'
					: 'Try another filter or load older events.'}
			</p>
		</div>
	{:else}
		{#each groups as group (group.day)}
			<section class="rounded-lg border bg-card px-4 py-2" aria-label={group.day}>
				<h3 class="pt-1 pb-1 text-xs font-medium text-muted-foreground">{group.day}</h3>
				<ul class="divide-y divide-dashed">
					{#each group.events as event (event.at + event.kind + event.title)}
						<AppEventRow {event} {appId} {now} />
					{/each}
				</ul>
			</section>
		{/each}
	{/if}

	{#if events && events.length > 0}
		<div class="flex justify-center">
			{#if nextBefore}
				<Button variant="outline" size="sm" onclick={handleLoadMore} disabled={loadingMore}>
					{loadingMore ? 'Loading…' : 'Load older events'}
				</Button>
			{:else}
				<p class="text-xs text-muted-foreground">
					That's everything kept. Events are kept for 90 days by default (Settings → Data
					retention).
				</p>
			{/if}
		</div>
	{/if}
</div>
