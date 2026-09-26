<!--
  UptimePanel — everything about one uptime check: current status, uptime over
  24 h / 7 d / 30 d, TLS and DNS state, status strips (24 h and 90 days),
  response times, incident history and the check's settings.

  <UptimePanel target={app.id} canEdit={can('apps:write')} />   an app
  <UptimePanel target={null} canEdit={can('settings:write')} /> the dashboard
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		checkUptimeNow,
		getUptime,
		updateUptime,
		type MetricEvent,
		type UptimeCheck
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import {
		INTERVAL_OPTIONS,
		causeLabel,
		formatDuration,
		formatUptime,
		relativeUnix,
		statusText,
		validateCheckForm
	} from '$lib/uptime';
	import { niceMax } from '$lib/metrics-format';
	import { cn, safeUrl } from '$lib/utils';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import TimeChart from '$lib/components/metrics/time-chart.svelte';
	import UptimeBars from './UptimeBars.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import * as Empty from '$lib/components/ui/empty/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import RefreshIcon from '~icons/ri/refresh-line';
	import PulseIcon from '~icons/ri/pulse-line';
	import ExternalIcon from '~icons/ri/external-link-line';

	interface Props {
		/** App id or name; null for the dashboard's own check. */
		target: string | null;
		canEdit: boolean;
	}

	let { target, canEdit }: Props = $props();

	let data = $state<UptimeCheck | null>(null);
	let loadError = $state('');
	let now = $state(Date.now() / 1000);

	async function load() {
		try {
			data = await getUptime(target);
			loadError = '';
		} catch (e: any) {
			loadError = e.message ?? 'request failed';
		} finally {
			now = Date.now() / 1000;
		}
	}

	$effect(() => {
		void target;
		return pollWhileVisible(() => untrack(load), 60_000);
	});

	// ── Check now ──
	const checkNow = useAction(() => checkUptimeNow(target), {
		onSuccess: (fresh) => {
			data = fresh;
			now = Date.now() / 1000;
		}
	});

	// ── Settings form ──
	let enabled = $state(true);
	let path = $state('/');
	let interval = $state('60');
	let expectedStatus = $state('');
	let timeoutSecs = $state(10);
	let formError = $state('');
	let saved = $state('');
	let formFor = $state<number | null>(null);

	// Fill the form from the check once (and when another check is shown).
	$effect(() => {
		const c = data?.check;
		if (!c || formFor === c.id) return;
		formFor = c.id;
		enabled = c.enabled;
		path = c.path;
		interval = String(c.interval_secs);
		expectedStatus = c.expected_status === null ? '' : String(c.expected_status);
		timeoutSecs = c.timeout_secs;
	});

	const save = useAction(
		() =>
			updateUptime(target, {
				enabled,
				path: path.trim(),
				interval_secs: Number(interval),
				expected_status: expectedStatus.trim() === '' ? null : Number(expectedStatus),
				timeout_secs: timeoutSecs
			}),
		{
			onSuccess: (fresh) => {
				data = fresh;
				saved = enabled ? 'Saved. The next check uses these settings.' : 'Saved. Checks are off.';
			}
		}
	);

	function handleSave(e: Event) {
		e.preventDefault();
		saved = '';
		formError = validateCheckForm({ path, expectedStatus, timeoutSecs });
		if (formError) return;
		save.run();
	}

	let intervalLabel = $derived(
		INTERVAL_OPTIONS.find((o) => String(o.value) === interval)?.label ?? `${interval} s`
	);
	let last = $derived(data?.last ?? null);
	let latencyMax = $derived(niceMax(Math.max(100, ...(data?.latency.ms ?? [0])) * 1.1));
	let noEvents: MetricEvent[] = [];
	let noun = $derived(target === null ? 'the dashboard' : 'this app');
</script>

<div class="space-y-4 pt-4">
	{#if loadError && !data}
		<InlineError
			message={`Couldn't load the uptime check: ${loadError}. It retries every minute.`}
		/>
	{/if}

	{#if !data}
		<div class="grid grid-cols-2 gap-3 lg:grid-cols-4" aria-busy="true">
			{#each [0, 1, 2, 3] as i (i)}
				<Skeleton class="h-20" />
			{/each}
		</div>
		<Skeleton class="h-24" />
	{:else if !data.check}
		<Empty.Root class="border border-dashed">
			<Empty.Header>
				<Empty.Media variant="icon"><PulseIcon /></Empty.Media>
				<Empty.Title>No uptime check yet</Empty.Title>
				<Empty.Description>{data.paused_reason}</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<!-- Status -->
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div class="min-w-0 space-y-1">
				<div class="flex flex-wrap items-center gap-2">
					<StatusBadge
						status={data.status}
						label={data.status === 'down' && data.down_since
							? `Down for ${formatDuration(now - data.down_since)}`
							: statusText(data.status)}
					/>
					{#if data.url}
						<a
							href={safeUrl(data.url)}
							target="_blank"
							rel="noopener noreferrer"
							class="flex min-w-0 items-center gap-1 font-mono text-xs break-all text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
						>
							{data.url}<ExternalIcon class="size-3 shrink-0" aria-hidden="true" />
						</a>
					{/if}
				</div>
				<p class="text-xs text-muted-foreground">
					{#if data.status === 'paused'}
						{data.paused_reason}
					{:else}
						Checked {relativeUnix(last?.at, now)}{data.next_at
							? ` · next check ${relativeUnix(data.next_at, now)}`
							: ''} · from this server, through DNS, TLS and the proxy like a visitor
					{/if}
				</p>
			</div>
			{#if data.status !== 'paused'}
				<Button
					size="sm"
					variant="outline"
					onclick={() => checkNow.run()}
					disabled={checkNow.pending}
				>
					<RefreshIcon
						class={cn('size-4', checkNow.pending && 'animate-spin')}
						aria-hidden="true"
					/>
					{checkNow.pending ? 'Checking…' : 'Check now'}
				</Button>
			{/if}
		</div>
		<InlineError message={checkNow.error} ondismiss={checkNow.clear} />

		{#if data.status === 'down' && last}
			<div
				class="rounded-md border border-destructive/40 bg-destructive/10 p-3 text-sm"
				role="status"
			>
				<p class="font-medium text-destructive">{causeLabel(last.cause)}</p>
				<p class="text-foreground">{last.detail}</p>
			</div>
		{/if}

		<!-- Numbers -->
		<div class="grid grid-cols-2 gap-3 lg:grid-cols-4">
			<div class="rounded-lg border p-3">
				<p class="text-xs text-muted-foreground">Uptime · 24 h</p>
				<p class="text-xl font-semibold tabular-nums">{formatUptime(data.uptime.day)}</p>
				<p class="text-xs text-muted-foreground">
					{data.uptime.day_probes} check{data.uptime.day_probes === 1 ? '' : 's'}
				</p>
			</div>
			<div class="rounded-lg border p-3">
				<p class="text-xs text-muted-foreground">7 days · 30 days</p>
				<p class="text-xl font-semibold tabular-nums">{formatUptime(data.uptime.week)}</p>
				<p class="text-xs text-muted-foreground tabular-nums">
					{formatUptime(data.uptime.month)} over 30 days
				</p>
			</div>
			<div class="rounded-lg border p-3">
				<p class="text-xs text-muted-foreground">Response time</p>
				<p class="text-xl font-semibold tabular-nums">
					{last?.ok ? `${last.latency_ms} ms` : '—'}
				</p>
				<p class="text-xs text-muted-foreground">
					{last?.status ? `HTTP ${last.status}` : 'No answer'}
				</p>
			</div>
			<div
				class={cn(
					'rounded-lg border p-3',
					last?.cert_days_left !== null &&
						last?.cert_days_left !== undefined &&
						last.cert_days_left < 14 &&
						'border-warn/50 bg-warn/5'
				)}
			>
				<p class="text-xs text-muted-foreground">TLS certificate</p>
				<p class="text-xl font-semibold tabular-nums">
					{last?.cert_days_left !== null && last?.cert_days_left !== undefined
						? last.cert_days_left < 0
							? 'Expired'
							: `${last.cert_days_left} days left`
						: '—'}
				</p>
				<p class="truncate text-xs text-muted-foreground" title={last?.dns_label}>
					DNS: {last?.dns_label ?? 'not checked yet'}{last?.ip ? ` (${last.ip})` : ''}
				</p>
			</div>
		</div>

		<!-- Strips -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Last 24 hours</Card.Title>
				<Card.Description>Each bar is 30 minutes. Hover a bar for its checks.</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-6">
				<div>
					<UptimeBars bars={data.recent} label="Last 24 hours" />
					<div class="mt-1 flex justify-between text-[10px] text-muted-foreground">
						<span>24 h ago</span><span>now</span>
					</div>
				</div>
				<div>
					<p class="mb-2 text-sm font-medium">Last 90 days</p>
					<UptimeBars bars={data.daily} width="day" label="Last 90 days" class="h-6" />
					<div class="mt-1 flex justify-between text-[10px] text-muted-foreground">
						<span>90 days ago</span><span>today</span>
					</div>
				</div>
				<div class="flex flex-wrap gap-4 text-xs text-muted-foreground">
					<span class="flex items-center gap-1.5"
						><span class="size-2 rounded-sm bg-success"></span>All checks passed</span
					>
					<span class="flex items-center gap-1.5"
						><span class="size-2 rounded-sm bg-warn"></span>Some failed</span
					>
					<span class="flex items-center gap-1.5"
						><span class="size-2 rounded-sm bg-destructive"></span>Mostly down</span
					>
					<span class="flex items-center gap-1.5"
						><span class="size-2 rounded-sm bg-border"></span>Not checked</span
					>
				</div>
			</Card.Content>
		</Card.Root>

		<!-- Response time -->
		{#if data.latency.t.length > 1}
			<Card.Root>
				<Card.Header>
					<Card.Title>Response time</Card.Title>
					<Card.Description
						>Time to the first byte of the answer, successful checks, 10-minute averages</Card.Description
					>
				</Card.Header>
				<Card.Content>
					<TimeChart
						label="Response time"
						times={data.latency.t}
						series={[
							{
								key: 'ms',
								label: 'Response time',
								values: data.latency.ms,
								color: 'var(--chart-1)',
								area: true
							}
						]}
						from={Math.floor(now / 600) * 600 - 86_400}
						to={now}
						step={600}
						range="24h"
						yMax={latencyMax}
						yFormat={(v) => `${Math.round(v)} ms`}
						events={noEvents}
						height={160}
					/>
				</Card.Content>
			</Card.Root>
		{/if}

		<!-- Incidents -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Incidents</Card.Title>
				<Card.Description>When {noun} was down, and why. Kept for 90 days.</Card.Description>
			</Card.Header>
			<Card.Content>
				{#if data.incidents.length === 0}
					<p class="text-sm text-muted-foreground">No incidents in the last 90 days.</p>
				{:else}
					<div class="overflow-x-auto rounded-md border">
						<table class="w-full text-sm">
							<caption class="sr-only">Incidents</caption>
							<thead>
								<tr class="border-b bg-muted/50 text-left">
									<th class="px-3 py-2 font-medium text-muted-foreground">Started</th>
									<th class="px-3 py-2 font-medium text-muted-foreground">Down for</th>
									<th class="px-3 py-2 font-medium text-muted-foreground">Cause</th>
									<th class="hidden px-3 py-2 font-medium text-muted-foreground md:table-cell"
										>What happened</th
									>
								</tr>
							</thead>
							<tbody>
								{#each data.incidents as incident (incident.id)}
									<tr class="border-b align-top last:border-0">
										<td class="px-3 py-2 whitespace-nowrap">
											{new Date(incident.started_at * 1000).toLocaleString([], {
												month: 'short',
												day: 'numeric',
												hour: '2-digit',
												minute: '2-digit'
											})}
										</td>
										<td class="px-3 py-2 whitespace-nowrap tabular-nums">
											{#if incident.ended_at === null}
												<span class="text-destructive"
													>{formatDuration(now - incident.started_at)} · ongoing</span
												>
											{:else}
												{formatDuration(incident.duration_secs)}
											{/if}
										</td>
										<td class="px-3 py-2">
											{causeLabel(incident.cause)}
											<p class="mt-1 text-xs text-muted-foreground md:hidden">{incident.detail}</p>
										</td>
										<td class="hidden px-3 py-2 text-muted-foreground md:table-cell"
											>{incident.detail}</td
										>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</Card.Content>
		</Card.Root>

		<!-- Settings -->
		<Card.Root>
			<Card.Header>
				<Card.Title>Check settings</Card.Title>
				<Card.Description>
					A failed check is retried once after 3 seconds before it counts, so one lost packet
					doesn't open an incident.
				</Card.Description>
			</Card.Header>
			<Card.Content>
				<form class="space-y-4" onsubmit={handleSave} novalidate>
					<div class="flex items-center gap-2">
						<Checkbox id="uptime-enabled" bind:checked={enabled} disabled={!canEdit} />
						<Label for="uptime-enabled">Check {noun} {intervalLabel.toLowerCase()}</Label>
					</div>
					<div class="grid max-w-3xl gap-4 sm:grid-cols-2 lg:grid-cols-4">
						<div class="grid gap-1.5 sm:col-span-2">
							<Label for="uptime-path">Path</Label>
							<Input
								id="uptime-path"
								bind:value={path}
								placeholder="/health"
								class="font-mono"
								disabled={!canEdit}
							/>
						</div>
						<div class="grid gap-1.5">
							<Label for="uptime-interval">Interval</Label>
							<Select.Root type="single" bind:value={interval} disabled={!canEdit}>
								<Select.Trigger id="uptime-interval" class="w-full">{intervalLabel}</Select.Trigger>
								<Select.Content>
									{#each INTERVAL_OPTIONS as option (option.value)}
										<Select.Item value={String(option.value)} label={option.label} />
									{/each}
								</Select.Content>
							</Select.Root>
						</div>
						<div class="grid gap-1.5">
							<Label for="uptime-timeout">Timeout (seconds)</Label>
							<Input
								id="uptime-timeout"
								type="number"
								min={1}
								max={30}
								bind:value={timeoutSecs}
								disabled={!canEdit}
							/>
						</div>
						<div class="grid gap-1.5 sm:col-span-2">
							<Label for="uptime-status">Expected status</Label>
							<Input
								id="uptime-status"
								inputmode="numeric"
								bind:value={expectedStatus}
								placeholder="Any 2xx or 3xx"
								disabled={!canEdit}
							/>
						</div>
					</div>
					<InlineError
						message={formError || save.error}
						ondismiss={() => ((formError = ''), save.clear())}
					/>
					<SuccessAlert message={saved} timeout={4000} ondismiss={() => (saved = '')} />
					{#if canEdit}
						<Button type="submit" size="sm" disabled={save.pending}>
							{save.pending ? 'Saving…' : 'Save'}
						</Button>
					{:else}
						<p class="text-xs text-muted-foreground">
							Changing the check needs the {target === null ? 'settings:write' : 'apps:write'} permission.
						</p>
					{/if}
				</form>
			</Card.Content>
		</Card.Root>
	{/if}
</div>
