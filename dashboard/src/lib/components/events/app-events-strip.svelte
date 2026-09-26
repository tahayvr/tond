<!--
  AppEventsStrip — "Recent events" on the app Overview: the last few deploys,
  restarts, crashes, OOM kills, certificate and configuration changes.
  Self-contained: loads its own data and refreshes while the tab is visible.

  <AppEventsStrip appId={app.id} />
-->
<script lang="ts">
	import { getAppEvents, type AppEvent } from '$lib/api';
	import { problemCount } from '$lib/app-events';
	import { pollWhileVisible } from '$lib/polling';
	import { appTabHref } from '$lib/tabs';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import AppEventRow from './app-event-row.svelte';
	import ArrowRightIcon from '~icons/ri/arrow-right-line';

	let { appId, limit = 5 }: { appId: string; limit?: number } = $props();

	/** Events are rare; a minute keeps the strip current without chatter. */
	const REFRESH_MS = 60_000;

	let events = $state<AppEvent[] | null>(null);
	let error = $state('');
	let now = $state(Date.now());

	let problems = $derived(events ? problemCount(events) : 0);

	async function load(id: string) {
		try {
			const page = await getAppEvents(id, { limit });
			if (id !== appId) return;
			events = page.events;
			error = '';
		} catch (e: any) {
			error = e.message;
		} finally {
			now = Date.now();
		}
	}

	$effect(() => {
		const id = appId;
		events = null;
		return pollWhileVisible(() => load(id), REFRESH_MS);
	});
</script>

<section class="min-w-0 rounded border border-dotted p-4" aria-labelledby="recent-events-title">
	<div class="mb-1 flex items-center justify-between gap-3">
		<h2
			id="recent-events-title"
			class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
		>
			Recent events
			{#if problems > 0}
				<span class="ml-1 rounded-sm bg-destructive/10 px-1.5 py-0.5 text-destructive normal-case">
					{problems}
					{problems === 1 ? 'problem' : 'problems'}
				</span>
			{/if}
		</h2>
		<a
			href={appTabHref(appId, 'events')}
			class="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
		>
			All events <ArrowRightIcon class="size-3.5" aria-hidden="true" />
		</a>
	</div>

	<InlineError message={error} />

	{#if events === null && !error}
		<div class="space-y-2 pt-2">
			{#each Array(3) as _, i (i)}
				<Skeleton class="h-9" />
			{/each}
		</div>
	{:else if events && events.length === 0}
		<p class="py-3 text-sm text-muted-foreground">
			Nothing yet. Deploys, restarts, crashes and configuration changes show up here.
		</p>
	{:else if events}
		<ul class="divide-y divide-dashed">
			{#each events as event (event.at + event.kind + event.title)}
				<AppEventRow {event} {appId} {now} compact />
			{/each}
		</ul>
	{/if}
</section>
