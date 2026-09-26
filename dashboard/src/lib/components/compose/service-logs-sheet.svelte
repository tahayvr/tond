<script lang="ts">
	// Side panel with one compose service's recent output (scrubbed of the
	// app's secrets by the server).
	import { serviceLogs, type LogLine } from '$lib/api';
	import * as Sheet from '$lib/components/ui/sheet/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';

	interface Props {
		appId: string;
		service: string | null;
		onclose: () => void;
	}

	let { appId, service, onclose }: Props = $props();

	let lines = $state<LogLine[]>([]);
	let loading = $state(false);
	let error = $state('');

	/** Docker's "<RFC 3339 ts> <line>" into a log line. */
	function toLine(raw: string): LogLine {
		const space = raw.indexOf(' ');
		const ts = space > 0 ? raw.slice(0, space) : '';
		return /^\d{4}-\d{2}-\d{2}T/.test(ts)
			? { source: 'container', ts, text: raw.slice(space + 1) }
			: { source: 'container', text: raw };
	}

	async function load(name: string) {
		loading = true;
		error = '';
		try {
			const res = await serviceLogs(appId, name, 500);
			lines = res.lines.map(toLine);
		} catch (e: any) {
			error = e.message ?? 'Could not load the logs';
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (service) load(service);
		else lines = [];
	});
</script>

<Sheet.Root
	open={service !== null}
	onOpenChange={(open) => {
		if (!open) onclose();
	}}
>
	<Sheet.Content side="right" class="w-full gap-0 overflow-y-auto p-0 sm:max-w-3xl">
		{#if service}
			<Sheet.Header class="border-b px-4 py-3 pr-10 sm:px-6">
				<Sheet.Title class="truncate">Logs · <span class="font-mono">{service}</span></Sheet.Title>
				<Sheet.Description>The last 500 lines. Secrets of this app are masked.</Sheet.Description>
			</Sheet.Header>
			<div class="space-y-3 px-4 py-4 sm:px-6">
				<InlineError message={error} ondismiss={() => (error = '')} />
				<LogViewer
					{lines}
					downloadName="{service}-logs"
					emptyText={loading ? 'Loading…' : 'No output yet.'}
					heightClass="h-[calc(100vh-12rem)]"
					showPrefix={false}
					label="{service} logs"
				>
					{#snippet actions()}
						<Button
							variant="ghost"
							size="sm"
							class="h-7 px-2 text-[11px]"
							onclick={() => service && load(service)}
							disabled={loading}
							aria-busy={loading}
						>
							{loading ? 'Loading…' : 'Refresh'}
						</Button>
					{/snippet}
				</LogViewer>
			</div>
		{/if}
	</Sheet.Content>
</Sheet.Root>
