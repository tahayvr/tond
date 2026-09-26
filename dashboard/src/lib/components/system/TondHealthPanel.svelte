<script lang="ts">
	// Tond health: the services Tond depends on (tond-server, tond-proxy,
	// Docker, the database, the outbound heartbeat) and every background loop.
	// Polls GET /api/v1/system/self itself; Docker and heartbeat come from the
	// page (GET /api/v1/system/host, /api/v1/system/heartbeat).
	import { untrack } from 'svelte';
	import {
		getSystemSelf,
		type DockerHealth,
		type HeartbeatStatus,
		type LoopStats,
		type SystemSelf
	} from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { cn } from '$lib/utils';
	import { heartbeatLine } from '$lib/host-format';
	import {
		formatBytes,
		formatMs,
		formatPercent,
		formatUptime,
		loopHealth,
		loopHealthText,
		loopLabel,
		relativeTime
	} from '$lib/tondHealth';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import HeartPulse from '~icons/ri/heart-pulse-line';

	interface Props {
		/** Refresh interval while the tab is visible */
		intervalMs?: number;
		docker?: DockerHealth | null;
		heartbeat?: HeartbeatStatus | null;
		/** Where the heartbeat is configured. */
		heartbeatHref?: string;
	}

	let {
		intervalMs = 10_000,
		docker = null,
		heartbeat = null,
		heartbeatHref = '/settings#heartbeat'
	}: Props = $props();

	let data = $state<SystemSelf | null>(null);
	let error = $state('');
	let loading = $state(true);
	let now = $state(Date.now());

	async function load() {
		try {
			data = await getSystemSelf();
			error = '';
		} catch (e: any) {
			error = `Couldn't load Tond's own health: ${e.message ?? 'request failed'}. It retries every ${Math.round(intervalMs / 1000)} s.`;
		} finally {
			loading = false;
			now = Date.now();
		}
	}

	$effect(() => {
		void intervalMs;
		return pollWhileVisible(() => untrack(load), intervalMs);
	});

	interface LoopRow extends LoopStats {
		process: 'server' | 'proxy';
	}

	let loops = $derived.by<LoopRow[]>(() => {
		if (!data) return [];
		return [
			...data.loops.map((l) => ({ ...l, process: 'server' as const })),
			...(data.proxy.stats?.loops ?? []).map((l) => ({ ...l, process: 'proxy' as const }))
		];
	});
	let failing = $derived(loops.filter((l) => loopHealth(l) === 'failing').length);
	let proxy = $derived(data?.proxy.stats ?? null);
	let beat = $derived(heartbeat ? heartbeatLine(heartbeat, now / 1000) : null);

	const dot: Record<string, string> = {
		ok: 'bg-success',
		slow: 'bg-warn',
		failing: 'bg-destructive'
	};

	const TONE_STATUS: Record<string, string> = {
		success: 'healthy',
		warning: 'degraded',
		danger: 'failed',
		neutral: 'disabled'
	};
</script>

<section class="space-y-4" aria-labelledby="tond-health-title">
	<div class="flex flex-col gap-1 sm:flex-row sm:items-end sm:justify-between sm:gap-4">
		<div>
			<h2 id="tond-health-title" class="flex items-center gap-2 text-lg font-bold">
				<HeartPulse class="size-5 text-muted-foreground" aria-hidden="true" />
				Services
			</h2>
			<p class="text-sm text-muted-foreground">
				What Tond runs on and how its background work is doing.
			</p>
		</div>
		{#if data}
			<p class="font-mono text-[10px] text-muted-foreground">
				v{data.version} · up {formatUptime(data.server.uptime_secs)}
			</p>
		{/if}
	</div>

	{#if error}
		<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
			{error}
		</div>
	{/if}

	{#if loading && !data}
		<div
			class="h-48 animate-pulse rounded-lg border border-border bg-muted/40"
			aria-busy="true"
		></div>
	{:else if data}
		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm" data-testid="services">
				<caption class="sr-only">Services</caption>
				<thead>
					<tr class="border-b bg-muted/50 text-left">
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">Service</th>
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">Status</th>
						<th
							class="hidden px-3 py-2 text-right font-medium text-muted-foreground sm:table-cell sm:px-4"
							>Memory</th
						>
						<th
							class="hidden px-3 py-2 text-right font-medium text-muted-foreground sm:px-4 md:table-cell"
							>Up for</th
						>
					</tr>
				</thead>
				<tbody>
					<tr class="border-b" data-service="tond-server">
						<td class="px-3 py-2 sm:px-4">
							<p class="font-medium">tond-server</p>
							<p class="text-xs text-muted-foreground">
								CPU {formatPercent(data.server.cpu_percent)} · {data.server.tokio_alive_tasks} tasks ·
								builds
								{data.build_queue.active} running, {data.build_queue.queued} queued
							</p>
						</td>
						<td class="px-3 py-2 sm:px-4"><StatusBadge status="running" size="sm" /></td>
						<td class="hidden px-3 py-2 text-right tabular-nums sm:table-cell sm:px-4"
							>{formatBytes(data.server.rss_bytes)}</td
						>
						<td class="hidden px-3 py-2 text-right whitespace-nowrap sm:px-4 md:table-cell"
							>{formatUptime(data.server.uptime_secs)}</td
						>
					</tr>
					<tr class="border-b" data-service="tond-proxy">
						<td class="px-3 py-2 sm:px-4">
							<p class="font-medium">tond-proxy</p>
							<p class="text-xs text-muted-foreground">
								{#if proxy}
									CPU {formatPercent(data.proxy.cpu_percent)} · {proxy.routes} routes ·
								{/if}
								{data.proxy.connected
									? 'route changes apply instantly'
									: 'not connected: routes update within a minute'}
							</p>
						</td>
						<td class="px-3 py-2 sm:px-4">
							<StatusBadge
								status={data.proxy.connected ? 'connected' : 'degraded'}
								label={data.proxy.connected ? 'Connected' : 'Not connected'}
								size="sm"
							/>
						</td>
						<td class="hidden px-3 py-2 text-right tabular-nums sm:table-cell sm:px-4"
							>{proxy ? formatBytes(proxy.rss_bytes) : '–'}</td
						>
						<td class="hidden px-3 py-2 text-right whitespace-nowrap sm:px-4 md:table-cell"
							>{proxy ? formatUptime(proxy.uptime_secs) : '–'}</td
						>
					</tr>
					<tr class="border-b" data-service="docker">
						<td class="px-3 py-2 sm:px-4">
							<p class="font-medium">Docker</p>
							{#if docker?.reachable}
								<p class="text-xs text-muted-foreground">
									{docker.version ?? 'unknown version'} · {docker.storage_driver ??
										'unknown driver'}
									· {docker.containers_running ?? 0} of {docker.containers ?? 0} containers running ·
									{docker.images ?? 0} images
								</p>
								{#each docker.warnings as warning (warning)}
									<p class="text-xs text-warn">{warning}</p>
								{/each}
							{:else if docker}
								<p class="text-xs text-destructive">
									{docker.error ?? 'Docker is not answering'}. Apps can't be deployed or restarted.
									On the server: <code class="font-mono">systemctl status docker</code>
								</p>
							{:else}
								<p class="text-xs text-muted-foreground">Checking…</p>
							{/if}
						</td>
						<td class="px-3 py-2 sm:px-4">
							{#if docker}
								<StatusBadge
									status={docker.reachable ? 'healthy' : 'failed'}
									label={docker.reachable ? 'Reachable' : 'Unreachable'}
									size="sm"
								/>
							{/if}
						</td>
						<td class="hidden px-3 py-2 text-right tabular-nums sm:table-cell sm:px-4"
							>{formatBytes(docker?.rss_bytes)}</td
						>
						<td class="hidden px-3 py-2 text-right whitespace-nowrap sm:px-4 md:table-cell"
							>{docker?.uptime_secs ? formatUptime(docker.uptime_secs) : '–'}</td
						>
					</tr>
					<tr class="border-b" data-service="database">
						<td class="px-3 py-2 sm:px-4">
							<p class="font-medium">Database</p>
							<p class="text-xs text-muted-foreground">
								tond.db · WAL {formatBytes(data.db.wal_bytes)} · {data.db.slow_total} slow {data.db
									.slow_total === 1
									? 'query'
									: 'queries'}
							</p>
						</td>
						<td class="px-3 py-2 sm:px-4">
							<StatusBadge
								status={data.db.slow_total > 0 ? 'degraded' : 'healthy'}
								label={data.db.slow_total > 0 ? 'Slow queries' : 'Healthy'}
								size="sm"
							/>
						</td>
						<td class="hidden px-3 py-2 text-right tabular-nums sm:table-cell sm:px-4"
							>{formatBytes(data.db.size_bytes)}</td
						>
						<td class="hidden px-3 py-2 sm:px-4 md:table-cell"></td>
					</tr>
					<tr data-service="heartbeat">
						<td class="px-3 py-2 sm:px-4">
							<p class="font-medium">Heartbeat</p>
							<p class="text-xs text-muted-foreground">
								{#if beat}
									{beat.text}{heartbeat?.configured && heartbeat.enabled && heartbeat.next_at
										? ` · next ${relativeTime(new Date(heartbeat.next_at * 1000).toISOString(), now)}`
										: ''} ·
								{/if}
								<a href={heartbeatHref} class="underline underline-offset-4 hover:text-foreground"
									>{heartbeat?.configured ? 'Settings' : 'Set up a dead-man’s switch'}</a
								>
							</p>
						</td>
						<td class="px-3 py-2 sm:px-4">
							{#if beat}
								<StatusBadge
									status={TONE_STATUS[beat.tone]}
									label={beat.tone === 'success'
										? 'Sending'
										: beat.tone === 'warning'
											? 'Holding'
											: beat.tone === 'danger'
												? 'Failing'
												: heartbeat?.configured
													? 'Off'
													: 'Not set up'}
									size="sm"
								/>
							{/if}
						</td>
						<td class="hidden px-3 py-2 sm:table-cell sm:px-4"></td>
						<td class="hidden px-3 py-2 sm:px-4 md:table-cell"></td>
					</tr>
				</tbody>
			</table>
		</div>

		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm">
				<caption class="sr-only">Background loops</caption>
				<thead>
					<tr class="border-b bg-muted/50 text-left">
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">
							Background work
							{#if failing > 0}
								<span class="ml-1 text-destructive">· {failing} failing</span>
							{/if}
						</th>
						<th class="px-3 py-2 font-medium text-muted-foreground sm:px-4">Last run</th>
						<th class="hidden px-3 py-2 font-medium text-muted-foreground sm:table-cell sm:px-4"
							>Took</th
						>
						<th class="hidden px-3 py-2 font-medium text-muted-foreground sm:px-4 md:table-cell">
							Next check
						</th>
						<th class="px-3 py-2 text-right font-medium text-muted-foreground sm:px-4">Errors</th>
					</tr>
				</thead>
				<tbody>
					{#each loops as loop (`${loop.process}:${loop.name}`)}
						{@const health = loopHealth(loop)}
						<tr class="border-b last:border-0" data-loop={loop.name}>
							<td class="px-3 py-2 sm:px-4">
								<div class="flex items-center gap-2">
									<span
										class={cn('inline-block size-2 shrink-0 rounded-full', dot[health])}
										title={loopHealthText(health)}
										aria-label={loopHealthText(health)}
									></span>
									<span>{loopLabel(loop.name)}</span>
									{#if loop.process === 'proxy'}
										<span class="font-mono text-[10px] text-muted-foreground uppercase">proxy</span>
									{/if}
								</div>
								{#if loop.last_error}
									<p
										class="mt-1 max-w-md truncate text-xs text-destructive"
										title={loop.last_error}
									>
										{loop.last_error}
									</p>
								{/if}
							</td>
							<td class="px-3 py-2 whitespace-nowrap text-muted-foreground sm:px-4">
								{relativeTime(loop.last_run_at, now)}
							</td>
							<td class="hidden px-3 py-2 text-muted-foreground tabular-nums sm:table-cell sm:px-4">
								{formatMs(loop.last_duration_ms)}
							</td>
							<td
								class="hidden px-3 py-2 whitespace-nowrap text-muted-foreground sm:px-4 md:table-cell"
							>
								{loop.next_wake_at ? relativeTime(loop.next_wake_at, now) : 'on change'}
							</td>
							<td
								class={cn(
									'px-3 py-2 text-right tabular-nums sm:px-4',
									loop.errors > 0 ? 'text-destructive' : 'text-muted-foreground'
								)}
							>
								{loop.errors}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<p class="text-xs text-muted-foreground">
			Loops sleep until they have work, so “next check” can be hours away on a quiet server.
			Prometheus metrics: <code class="font-mono"
				>curl --unix-socket /opt/tond/data/tond.sock http://tond/metrics</code
			> on the server.
		</p>
	{/if}
</section>
