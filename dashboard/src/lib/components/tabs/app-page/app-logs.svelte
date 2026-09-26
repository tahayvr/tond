<script lang="ts">
	// App logs tab: deploy log lines and container output, live — or the
	// opt-in access log (`?log=access`).
	import { page } from '$app/state';
	import type { LogLine } from '$lib/api';
	import { APP_LOG_VIEWS, appLogViewFromUrl, appLogViewHref } from '$lib/tabs';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as ButtonGroup from '$lib/components/ui/button-group/index.js';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';
	import AppAccessLog from './app-access-log.svelte';

	interface Props {
		appId: string;
		appName: string;
		/** Compose app: offer a per-service filter (services seen in the output). */
		compose?: boolean;
		logLines: LogLine[];
		streaming: boolean;
		onConnect: () => void;
		onStop: () => void;
		onClear: () => void;
	}

	let {
		appId,
		appName,
		compose = false,
		logLines,
		streaming,
		onConnect,
		onStop,
		onClear
	}: Props = $props();

	let view = $derived(appLogViewFromUrl(page.url));
	let services = $derived(
		compose
			? [...new Set(logLines.map((l) => l.service).filter((s): s is string => !!s))].sort()
			: []
	);
	/** Compose apps: show one service's output ('' = every service and deploy lines). */
	let serviceFilter = $state('');
	let shownLines = $derived(
		serviceFilter ? logLines.filter((l) => l.service === serviceFilter) : logLines
	);
</script>

<div class="space-y-4 pt-4">
	<nav aria-label="Log type">
		<ButtonGroup.Root>
			{#each APP_LOG_VIEWS as v (v.id)}
				<Button
					size="sm"
					variant={view === v.id ? 'default' : 'outline'}
					href={appLogViewHref(appId, v.id)}
					aria-current={view === v.id ? 'page' : undefined}
				>
					{v.label}
				</Button>
			{/each}
		</ButtonGroup.Root>
	</nav>

	{#if view !== 'access' && services.length > 0}
		<div class="flex flex-wrap items-center gap-2" role="group" aria-label="Service">
			<span class="text-xs text-muted-foreground">Service:</span>
			<Button
				size="sm"
				variant={serviceFilter === '' ? 'default' : 'outline'}
				aria-pressed={serviceFilter === ''}
				onclick={() => (serviceFilter = '')}>All</Button
			>
			{#each services as name (name)}
				<Button
					size="sm"
					variant={serviceFilter === name ? 'default' : 'outline'}
					class="font-mono"
					aria-pressed={serviceFilter === name}
					onclick={() => (serviceFilter = name)}>{name}</Button
				>
			{/each}
		</div>
	{/if}

	{#if view === 'access'}
		<AppAccessLog {appId} {appName} />
	{:else}
		<LogViewer
			lines={shownLines}
			live={streaming}
			downloadName={serviceFilter ? `${appName}-${serviceFilter}-logs` : `${appName}-logs`}
			emptyText={streaming ? 'Waiting for output…' : 'Not connected. Connect to stream logs.'}
			heightClass="h-[26rem] sm:h-[34rem]"
			showPrefix={services.length > 0 && !serviceFilter}
			label="{appName} logs"
		>
			{#snippet actions()}
				{#if streaming}
					<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onStop}
						>Stop</Button
					>
				{:else}
					<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onConnect}
						>Connect</Button
					>
				{/if}
				<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onClear}
					>Clear</Button
				>
			{/snippet}
		</LogViewer>
	{/if}
</div>
