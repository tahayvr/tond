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
		logLines: LogLine[];
		streaming: boolean;
		onConnect: () => void;
		onStop: () => void;
		onClear: () => void;
	}

	let { appId, appName, logLines, streaming, onConnect, onStop, onClear }: Props = $props();

	let view = $derived(appLogViewFromUrl(page.url));
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

	{#if view === 'access'}
		<AppAccessLog {appId} {appName} />
	{:else}
		<LogViewer
			lines={logLines}
			live={streaming}
			downloadName="{appName}-logs"
			emptyText={streaming ? 'Waiting for output…' : 'Not connected. Connect to stream logs.'}
			heightClass="h-[26rem] sm:h-[34rem]"
			showPrefix={false}
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
