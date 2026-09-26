<!--
  UptimeList — every uptime check at a glance (GET /api/v1/uptime): status and
  uptime over 24 h / 7 d / 30 d, each app linking to its Uptime tab.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { listUptime, type UptimeSummary } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { appTabHref } from '$lib/tabs';
	import { formatDuration, formatUptime, statusText } from '$lib/uptime';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';

	let rows = $state<UptimeSummary[] | null>(null);
	let error = $state('');
	let now = $state(Date.now() / 1000);

	async function load() {
		try {
			rows = await listUptime();
			error = '';
		} catch (e: any) {
			error = `Couldn't load uptime checks: ${e.message ?? 'request failed'}`;
		} finally {
			now = Date.now() / 1000;
		}
	}

	$effect(() => pollWhileVisible(() => untrack(load), 60_000));

	let down = $derived(rows?.filter((r) => r.status === 'down').length ?? 0);
</script>

<section class="space-y-3" aria-labelledby="uptime-list-title">
	<div>
		<h2 id="uptime-list-title" class="text-lg font-bold">
			All checks{#if down > 0}<span class="ml-2 text-base font-medium text-destructive"
					>· {down} down</span
				>{/if}
		</h2>
		<p class="text-sm text-muted-foreground">
			Every app with a domain is checked from this server through DNS, TLS and the proxy, like a
			visitor would reach it.
		</p>
	</div>
	<InlineError message={error} />
	{#if !rows}
		<Skeleton class="h-32" />
	{:else if rows.length === 0}
		<p class="text-sm text-muted-foreground">No checks yet.</p>
	{:else}
		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm">
				<caption class="sr-only">Uptime checks</caption>
				<thead>
					<tr class="border-b bg-muted/50 text-left">
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">Name</th>
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">Status</th>
						<th class="px-3 py-2 text-right font-medium text-muted-foreground sm:px-4">24 h</th>
						<th
							class="hidden px-3 py-2 text-right font-medium text-muted-foreground sm:table-cell sm:px-4"
							>7 d</th
						>
						<th
							class="hidden px-3 py-2 text-right font-medium text-muted-foreground sm:px-4 md:table-cell"
							>30 d</th
						>
						<th
							class="hidden px-3 py-2 text-right font-medium text-muted-foreground sm:px-4 lg:table-cell"
							>Response</th
						>
					</tr>
				</thead>
				<tbody>
					{#each rows as row (row.check_id)}
						<tr class="border-b last:border-0">
							<td class="max-w-64 px-3 py-2 sm:px-4">
								{#if row.app_id}
									<a
										href={appTabHref(row.app_id, 'uptime')}
										class="font-medium underline-offset-4 hover:underline">{row.name}</a
									>
								{:else}
									<span class="font-medium">{row.name}</span>
								{/if}
								<p class="truncate font-mono text-xs text-muted-foreground">{row.host || '—'}</p>
							</td>
							<td class="px-3 py-2 sm:px-4">
								<StatusBadge
									status={row.status}
									size="sm"
									label={row.status === 'down' && row.down_since
										? `Down ${formatDuration(now - row.down_since)}`
										: statusText(row.status)}
								/>
							</td>
							<td class="px-3 py-2 text-right tabular-nums sm:px-4"
								>{formatUptime(row.uptime.day)}</td
							>
							<td class="hidden px-3 py-2 text-right tabular-nums sm:table-cell sm:px-4"
								>{formatUptime(row.uptime.week)}</td
							>
							<td class="hidden px-3 py-2 text-right tabular-nums sm:px-4 md:table-cell"
								>{formatUptime(row.uptime.month)}</td
							>
							<td
								class="hidden px-3 py-2 text-right text-muted-foreground tabular-nums sm:px-4 lg:table-cell"
								>{row.status === 'up' && row.latency_ms !== null ? `${row.latency_ms} ms` : '—'}</td
							>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>
