<script lang="ts">
	// A LogViewer fed by one of the log WebSockets (app, database, system).
	// The socket is open while this component is mounted and connected; lines
	// are bounded and batched by the log buffer.
	import { openLogStream, type LogLine } from '$lib/api';
	import { createLogBuffer } from '$lib/log-buffer';
	import { Button } from '$lib/components/ui/button/index.js';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';

	interface Props {
		/** e.g. `/api/databases/{id}/logs/stream` */
		wsUrl: string;
		downloadName: string;
		label: string;
		heightClass?: string;
		showPrefix?: boolean;
	}

	let {
		wsUrl,
		downloadName,
		label,
		heightClass = 'h-[26rem] sm:h-[32rem]',
		showPrefix = true
	}: Props = $props();

	let lines = $state.raw<LogLine[]>([]);
	let streaming = $state(false);
	let paused = $state(false);
	/** Bumped by Connect to open a fresh socket. */
	let attempt = $state(0);
	const buffer = createLogBuffer<LogLine>((next) => (lines = next));

	$effect(() => {
		const url = wsUrl;
		void attempt;
		if (paused) return;
		buffer.clear();
		streaming = true;
		const close = openLogStream(url, {
			onLine: (line) => buffer.push(line),
			onError: () => (streaming = false),
			onClose: () => {
				buffer.flush();
				streaming = false;
			}
		});
		return () => {
			close();
			buffer.flush();
			streaming = false;
		};
	});

	$effect(() => () => buffer.dispose());
</script>

<LogViewer
	{lines}
	live={streaming}
	{downloadName}
	{label}
	{heightClass}
	{showPrefix}
	emptyText={streaming ? 'Waiting for output…' : 'Not connected.'}
>
	{#snippet actions()}
		{#if streaming}
			<Button variant="ghost" size="sm" class="h-7 px-2 text-[11px]" onclick={() => (paused = true)}
				>Stop</Button
			>
		{:else}
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 text-[11px]"
				onclick={() => {
					paused = false;
					attempt += 1;
				}}>Connect</Button
			>
		{/if}
	{/snippet}
</LogViewer>
