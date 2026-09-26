<!-- AppEventRow — one event: icon, title, detail, who and when. -->
<script lang="ts">
	import type { AppEvent } from '$lib/api';
	import { formatRelativePast } from '$lib/time';
	import { cn } from '$lib/utils';
	import AppEventIcon from './app-event-icon.svelte';

	let {
		event,
		appId,
		now,
		compact = false
	}: { event: AppEvent; appId: string; now: number; compact?: boolean } = $props();

	let href = $derived(
		event.deployment_id ? `/apps/${appId}/deployments/${event.deployment_id}` : null
	);
</script>

<li class={cn('flex min-w-0 gap-3', compact ? 'py-2' : 'py-2.5')}>
	<AppEventIcon {event} />
	<div class="min-w-0 flex-1">
		<div class="flex min-w-0 items-baseline justify-between gap-3">
			<p class="min-w-0 truncate text-sm font-medium">
				{#if href}
					<a {href} class="hover:underline">{event.title}</a>
				{:else}
					{event.title}
				{/if}
			</p>
			<time
				datetime={event.at}
				title={new Date(event.at).toLocaleString()}
				class="shrink-0 text-xs text-muted-foreground tabular-nums"
				>{formatRelativePast(event.at, now)}</time
			>
		</div>
		{#if event.detail || event.actor}
			<p
				class={cn(
					'text-xs break-words text-muted-foreground',
					compact && 'truncate',
					event.severity === 'danger' && event.detail && 'text-destructive/90'
				)}
			>
				{#if event.detail}<span class="font-mono">{event.detail}</span
					>{/if}{#if event.detail && event.actor}<span aria-hidden="true">&nbsp;·&nbsp;</span
					>{/if}{#if event.actor}by {event.actor}{/if}
			</p>
		{/if}
	</div>
</li>
