<script lang="ts">
	import { tick } from 'svelte';
	import { getUiContext } from '$lib/ui-context.svelte';
	import { openLogStream, type LogLine } from '$lib/api';
	import { createLogBuffer } from '$lib/log-buffer';

	const MAX_LINES = 500;

	let ctx = $derived(getUiContext());
	let wsUrl = $derived(ctx.feed.wsUrl);

	// Human-readable idle message based on context type
	let idleMessage = $derived.by(() => {
		switch (ctx.type) {
			case 'project':
				return 'Open an individual app to stream its logs here.';
			case 'projects':
				return 'Waiting for system logs…';
			case 'idle':
				return 'Navigate to a resource to start a log stream.';
			default:
				return 'No log stream active.';
		}
	});

	// Bounded and batched: one array copy per flush, never more than MAX_LINES.
	let lines = $state.raw<LogLine[]>([]);
	const buffer = createLogBuffer<LogLine>((next) => (lines = next), { limit: MAX_LINES });
	let connected = $state(false);
	let reconnecting = $state(false);
	let autoScroll = $state(true);
	let scrollEl = $state<HTMLElement | null>(null);

	function levelClass(line: LogLine): string {
		if (line.source === 'deploy') {
			switch (line.level) {
				case 'error':
					return 'text-destructive';
				case 'warn':
					return 'text-warn';
				default:
					return 'text-muted-foreground';
			}
		}
		return 'text-foreground/80';
	}

	function prefix(line: LogLine): string {
		const app = line.appName ? `${line.appName} ` : '';
		return line.source === 'deploy' ? `${app}[${line.level ?? 'info'}]` : `${app}[out]`;
	}

	function scrollToBottom() {
		if (scrollEl) {
			scrollEl.scrollTop = scrollEl.scrollHeight;
		}
	}

	// Scroll to bottom after DOM updates whenever lines change and autoScroll is on
	$effect(() => {
		void lines.length;
		if (autoScroll) {
			tick().then(scrollToBottom);
		}
	});

	// Open / close WebSocket whenever wsUrl changes
	$effect(() => {
		const url = wsUrl;

		buffer.clear();
		reconnecting = false;
		connected = false;

		if (!url) return;

		const close = openLogStream(url, {
			onOpen(reconnected) {
				connected = true;
				reconnecting = false;
				// The server re-sends its snapshot on every connection.
				if (reconnected) buffer.clear();
			},
			onLine(line) {
				buffer.push(line);
			},
			onDisconnect() {
				connected = false;
				reconnecting = true;
			}
		});

		return () => {
			close();
			buffer.dispose();
		};
	});
</script>

<div class="relative flex h-full flex-col overflow-hidden font-mono text-xs">
	<!-- connection indicator bar -->
	<div class="flex shrink-0 items-center gap-1.5 border-b px-3 py-1.5 text-muted-foreground">
		{#if wsUrl}
			{#if connected}
				<span class="size-1.5 rounded-full bg-success"></span>
				<span>connected</span>
			{:else if reconnecting}
				<span class="size-1.5 animate-pulse rounded-full bg-destructive"></span>
				<span>connection lost, reconnecting…</span>
			{:else}
				<span class="size-1.5 animate-pulse rounded-full bg-warn"></span>
				<span>connecting…</span>
			{/if}
		{:else}
			<span class="size-1.5 rounded-full bg-muted-foreground/40"></span>
			<span>idle</span>
		{/if}

		<button
			class="ml-auto text-muted-foreground hover:text-foreground"
			onclick={() => (autoScroll = !autoScroll)}
			title={autoScroll ? 'Disable auto-scroll' : 'Enable auto-scroll'}
		>
			{autoScroll ? 'auto ↓' : 'manual'}
		</button>
	</div>

	<!-- log lines -->
	<div
		bind:this={scrollEl}
		onscroll={() => {
			if (!scrollEl) return;
			autoScroll = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 40;
		}}
		class="flex-1 overflow-y-auto px-3 py-2"
	>
		{#if lines.length === 0}
			<p class="text-muted-foreground select-none">
				{wsUrl ? 'Waiting for logs…' : idleMessage}
			</p>
		{:else}
			{#each lines as line, i (i)}
				<div class="mb-1 leading-5">
					<div class="flex gap-2 text-muted-foreground">
						{#if line.ts}
							<span class="shrink-0 text-muted-foreground tabular-nums">
								{new Date(line.ts).toLocaleTimeString()}
							</span>
						{/if}
						<span class="shrink-0">{prefix(line)}</span>
					</div>
					<div class="pl-1 break-all {levelClass(line)}">{line.text}</div>
				</div>
			{/each}
		{/if}
	</div>
</div>
