<script lang="ts">
	// Opt-in access log of an app: turn it on or off (with the storage note),
	// read the newest lines in the shared log viewer, search the whole file on
	// the server, download it, or delete it.
	import { untrack } from 'svelte';
	import {
		accessLogDownloadUrl,
		deleteAccessLog,
		getAccessLog,
		updateAccessLog,
		type AccessLog,
		type LogLine
	} from '$lib/api';
	import { can } from '$lib/auth.svelte';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { accessLogLines } from '$lib/access-log';
	import { formatBytes } from '$lib/metrics-format';
	import { pollWhileVisible } from '$lib/polling';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import LogViewer from '$lib/components/logs/log-viewer.svelte';
	import DownloadIcon from '~icons/ri/download-2-line';
	import SearchIcon from '~icons/ri/search-line';

	interface Props {
		appId: string;
		appName: string;
	}

	let { appId, appName }: Props = $props();

	const LINES = 1000;

	let log = $state<AccessLog | null>(null);
	let loadError = $state('');
	let query = $state('');
	/** The search the lines on screen were loaded with. */
	let activeQuery = $state('');
	let includeQuery = $state(false);

	let canEdit = $derived(can('apps:write'));
	let lines = $derived<LogLine[]>(log ? accessLogLines(log.lines) : []);
	let usedPct = $derived(log ? Math.min(100, (log.size_bytes / log.max_bytes) * 100) : 0);

	async function load() {
		try {
			const next = await getAccessLog(appId, { lines: LINES, q: activeQuery || undefined });
			log = next;
			includeQuery = next.include_query;
			loadError = '';
		} catch (e: any) {
			loadError = e.message ?? 'Could not load the access log';
		}
	}

	$effect(() => {
		void appId;
		void activeQuery;
		return pollWhileVisible(() => untrack(load), 15_000);
	});

	const save = useAction(async (enabled: boolean, withQuery: boolean) => {
		const saved = await updateAccessLog(appId, { enabled, include_query: withQuery });
		if (log) log = { ...log, enabled: saved.enabled, include_query: saved.include_query };
		includeQuery = saved.include_query;
	});

	function handleSearch(e: SubmitEvent) {
		e.preventDefault();
		activeQuery = query.trim();
	}

	function handleClearSearch() {
		query = '';
		activeQuery = '';
	}

	function handleDelete() {
		return confirmAction({
			title: `Delete the access log of ${appName}?`,
			description: 'The recorded requests are removed from the server.',
			details: [
				`${log ? formatBytes(log.size_bytes) : 'All'} of access log lines`,
				log?.enabled ? 'Recording continues with an empty log.' : 'Recording stays off.'
			],
			confirmLabel: 'Delete log',
			pendingLabel: 'Deleting…',
			onconfirm: async () => {
				await deleteAccessLog(appId);
				await load();
			}
		});
	}
</script>

<div class="space-y-4">
	<section class="space-y-3 rounded-lg border p-4" aria-labelledby="access-log-heading">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div class="min-w-0 space-y-1">
				<h2 id="access-log-heading" class="flex items-center gap-2 text-sm font-medium">
					Access log
					{#if log}
						<StatusBadge
							status={log.enabled ? 'enabled' : 'disabled'}
							label={log.enabled ? 'Recording' : 'Off'}
							size="sm"
						/>
					{/if}
				</h2>
				<p class="max-w-prose text-sm text-muted-foreground">
					One line per request: time, method, path, status, duration, response size and client IP.
					Uses up to {log ? formatBytes(log.max_bytes, 0) : '60 MB'} of disk for this app; when full,
					the oldest half is dropped.
				</p>
			</div>
			{#if log && canEdit}
				{#if log.enabled}
					<Button
						variant="outline"
						size="sm"
						disabled={save.pending}
						onclick={() => save.run(false, false)}
					>
						{save.pending ? 'Turning off…' : 'Turn off'}
					</Button>
				{:else}
					<Button size="sm" disabled={save.pending} onclick={() => save.run(true, includeQuery)}>
						{save.pending ? 'Turning on…' : 'Turn on access log'}
					</Button>
				{/if}
			{/if}
		</div>

		{#if log && canEdit}
			<div class="flex items-start gap-2">
				<Checkbox
					id="access-log-query"
					checked={includeQuery}
					disabled={save.pending}
					onCheckedChange={(v) => {
						includeQuery = v === true;
						if (log?.enabled) save.run(true, includeQuery);
					}}
				/>
				<div class="grid gap-0.5">
					<Label for="access-log-query" class="text-sm font-normal">Include query strings</Label>
					<p class="text-xs text-muted-foreground">
						Off by default: query strings often carry tokens and personal data.
					</p>
				</div>
			</div>
		{:else if log && !canEdit}
			<p class="text-xs text-muted-foreground">
				Turning the access log on or off needs the <code>apps:write</code> permission.
			</p>
		{/if}

		{#if log && (log.size_bytes > 0 || log.enabled)}
			<div class="space-y-1">
				<div
					class="h-1.5 overflow-hidden rounded-full bg-muted"
					role="progressbar"
					aria-label="Access log disk use"
					aria-valuemin={0}
					aria-valuemax={100}
					aria-valuenow={Math.round(usedPct)}
				>
					<div class="h-full bg-info" style="width: {usedPct}%"></div>
				</div>
				<p class="text-xs text-muted-foreground tabular-nums">
					{formatBytes(log.size_bytes)} of {formatBytes(log.max_bytes, 0)} used
				</p>
			</div>
		{/if}

		<InlineError message={save.error} ondismiss={save.clear} />
	</section>

	<InlineError title="Couldn't load the access log" message={loadError}>
		{#snippet actions()}
			<Button size="sm" variant="outline" onclick={load}>Retry</Button>
		{/snippet}
	</InlineError>

	{#if !log && !loadError}
		<Skeleton class="h-[26rem]" />
	{:else if log}
		<form class="flex flex-wrap items-center gap-2" onsubmit={handleSearch} role="search">
			<div class="relative min-w-0 flex-1 sm:max-w-sm">
				<SearchIcon
					class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
					aria-hidden="true"
				/>
				<Input
					bind:value={query}
					placeholder="Search the whole log"
					aria-label="Search the whole access log"
					class="pl-8"
				/>
			</div>
			<Button type="submit" size="sm" variant="outline">Search</Button>
			{#if activeQuery}
				<Button type="button" size="sm" variant="ghost" onclick={handleClearSearch}>Clear</Button>
			{/if}
		</form>
		{#if activeQuery}
			<p class="text-xs text-muted-foreground" role="status">
				{log.lines.length} matching line{log.lines.length === 1 ? '' : 's'} for “{activeQuery}”{log.truncated
					? ` (the newest ${LINES})`
					: ''}
			</p>
		{:else if log.truncated}
			<p class="text-xs text-muted-foreground">
				Showing the newest {LINES} lines. Search or download for older ones.
			</p>
		{/if}

		<LogViewer
			{lines}
			downloadName="{appName}-access"
			emptyText={activeQuery
				? 'No lines match this search.'
				: log.enabled
					? 'Recording. Requests to this app appear here within seconds.'
					: 'The access log is off. Turn it on to record requests.'}
			heightClass="h-[26rem] sm:h-[30rem]"
			showPrefix={false}
			label="{appName} access log"
		>
			{#snippet actions()}
				<Button
					variant="ghost"
					size="sm"
					class="h-7 px-2 text-[11px]"
					href={accessLogDownloadUrl(appId)}
					download="{appName}-access.log"
					disabled={log?.size_bytes === 0}
				>
					<DownloadIcon class="size-3.5" aria-hidden="true" /> Full log
				</Button>
				{#if canEdit && (log?.size_bytes ?? 0) > 0}
					<Button
						variant="ghost"
						size="sm"
						class="h-7 px-2 text-[11px] text-destructive hover:text-destructive"
						onclick={handleDelete}
					>
						Delete
					</Button>
				{/if}
			{/snippet}
		</LogViewer>
	{/if}
</div>
