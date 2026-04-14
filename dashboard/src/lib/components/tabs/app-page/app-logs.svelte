<script lang="ts" module>
	export interface LogLine {
		kind: 'deploy' | 'container';
		level?: string;
		trigger_source?: string;
		message?: string;
		ts?: string;
		line?: string;
	}
</script>

<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';

	interface Props {
		logLines: LogLine[];
		streaming: boolean;
		onConnect: () => void;
		onStop: () => void;
		onClear: () => void;
	}

	let { logLines, streaming, onConnect, onStop, onClear }: Props = $props();

	let logContainer = $state<HTMLElement | null>(null);

	$effect(() => {
		// Auto-scroll when new logs arrive
		if (logContainer && logLines.length > 0) {
			setTimeout(() => {
				if (logContainer) logContainer.scrollTop = logContainer.scrollHeight;
			}, 0);
		}
	});

	function fmtTs(ts: string | undefined): string {
		if (!ts) return '';
		try {
			return new Date(ts).toLocaleTimeString();
		} catch {
			return ts;
		}
	}
</script>

<div class="space-y-4 pt-4">
	<div class="flex items-center gap-2">
		{#if streaming}
			<Button variant="outline" size="sm" onclick={onStop}>Stop</Button>
			<span class="flex items-center gap-1 text-xs text-muted-foreground">
				<span class="inline-block h-2 w-2 animate-pulse rounded-full bg-success"></span>
				Live
			</span>
		{:else}
			<Button variant="outline" size="sm" onclick={onConnect}>Connect</Button>
		{/if}
		<Button variant="ghost" size="sm" onclick={onClear} class="text-muted-foreground">Clear</Button>
	</div>

	{#if logLines.length > 0}
		<div
			bind:this={logContainer}
			class="max-h-128 overflow-y-auto rounded-lg border bg-black p-4 font-mono text-sm"
		>
			{#each logLines as entry, i (i)}
				{#if entry.kind === 'deploy'}
					<div class="py-0.5">
						<span
							class={entry.level === 'error'
								? 'text-destructive'
								: entry.level === 'warn'
									? 'text-warn'
									: 'text-success'}>[{entry.level}]</span
						>
						{#if entry.trigger_source}
							<span class="text-secondary">[{entry.trigger_source}]</span>
						{/if}
						<span class="text-muted-foreground"> {fmtTs(entry.ts)} </span>
						<span class="text-foreground">{entry.message}</span>
					</div>
				{:else}
					<div class="py-0.5 whitespace-pre-wrap text-muted-foreground">
						{#if entry.ts}<span class="text-muted-foreground/50">{fmtTs(entry.ts)} </span>{/if}<span
							class="text-foreground">{entry.line}</span
						>
					</div>
				{/if}
			{/each}
		</div>
	{:else if !streaming}
		<div class="py-8 text-center text-muted-foreground">Click Connect to start streaming logs.</div>
	{:else}
		<div class="py-8 text-center text-muted-foreground">Waiting for logs...</div>
	{/if}
</div>
