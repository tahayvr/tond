<script lang="ts">
	// App logs tab: deploy log lines and container output, live.
	import type { LogLine } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';

	interface Props {
		appName: string;
		logLines: LogLine[];
		streaming: boolean;
		onConnect: () => void;
		onStop: () => void;
		onClear: () => void;
	}

	let { appName, logLines, streaming, onConnect, onStop, onClear }: Props = $props();
</script>

<div class="pt-4">
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
				<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onStop}>Stop</Button
				>
			{:else}
				<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onConnect}
					>Connect</Button
				>
			{/if}
			<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={onClear}>Clear</Button
			>
		{/snippet}
	</LogViewer>
</div>
