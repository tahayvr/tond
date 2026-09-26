<!--
  UptimeBadge — compact uptime summary for an app's Overview: status, uptime
  over 24 h / 7 d / 30 d, a 24-hour status strip and a link to the Uptime tab.
  Loads itself and refreshes every minute while the tab is visible.

  <UptimeBadge appId={app.id} href={appTabHref(app.id, 'uptime')} />
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { getUptime, type UptimeCheck } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { formatDuration, formatUptime, statusText } from '$lib/uptime';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import UptimeBars from './UptimeBars.svelte';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import ArrowRight from '~icons/ri/arrow-right-s-line';

	interface Props {
		appId: string;
		/** Link to the full uptime view. */
		href: string;
	}

	let { appId, href }: Props = $props();

	let data = $state<UptimeCheck | null>(null);
	let error = $state('');
	let now = $state(Date.now() / 1000);

	async function load() {
		try {
			data = await getUptime(appId);
			error = '';
		} catch (e: any) {
			error = e.message ?? 'request failed';
		} finally {
			now = Date.now() / 1000;
		}
	}

	$effect(() => {
		void appId;
		return pollWhileVisible(() => untrack(load), 60_000);
	});

	let last = $derived(data?.last ?? null);
</script>

<section
	class="min-w-0 space-y-3 rounded border border-dotted p-4"
	aria-labelledby="uptime-badge-title"
	data-testid="uptime-badge"
>
	<div class="flex items-center justify-between gap-2">
		<h2
			id="uptime-badge-title"
			class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
		>
			Uptime
		</h2>
		<a
			{href}
			class="flex items-center gap-0.5 text-xs text-muted-foreground underline-offset-4 hover:text-foreground hover:underline"
		>
			Details<ArrowRight class="size-4" aria-hidden="true" />
		</a>
	</div>

	{#if error && !data}
		<p class="text-sm text-muted-foreground">Uptime isn't available right now ({error}).</p>
	{:else if !data}
		<Skeleton class="h-5 w-64" />
		<Skeleton class="h-8" />
	{:else if !data.check}
		<p class="text-sm text-muted-foreground">{data.paused_reason}</p>
	{:else}
		<div class="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm">
			<StatusBadge
				status={data.status}
				label={data.status === 'down' && data.down_since
					? `Down for ${formatDuration(now - data.down_since)}`
					: statusText(data.status)}
			/>
			<span>
				<span class="font-semibold tabular-nums">{formatUptime(data.uptime.day)}</span>
				<span class="text-muted-foreground">24 h</span>
			</span>
			<span>
				<span class="font-semibold tabular-nums">{formatUptime(data.uptime.week)}</span>
				<span class="text-muted-foreground">7 d</span>
			</span>
			<span>
				<span class="font-semibold tabular-nums">{formatUptime(data.uptime.month)}</span>
				<span class="text-muted-foreground">30 d</span>
			</span>
			{#if last?.ok}
				<span class="text-muted-foreground tabular-nums">{last.latency_ms} ms</span>
			{/if}
			{#if last?.cert_days_left !== null && last?.cert_days_left !== undefined}
				<span class={last.cert_days_left < 14 ? 'text-warn' : 'text-muted-foreground'}>
					TLS: {last.cert_days_left} days left
				</span>
			{/if}
		</div>
		{#if data.status === 'down' && last}
			<p class="text-sm text-destructive">{last.detail}</p>
		{:else if data.status === 'paused' && data.paused_reason}
			<p class="text-sm text-muted-foreground">{data.paused_reason}</p>
		{/if}
		<div>
			<UptimeBars bars={data.recent} label="Last 24 hours" />
			<div class="mt-1 flex justify-between text-[10px] text-muted-foreground">
				<span>24 h ago</span>
				<span>now</span>
			</div>
		</div>
	{/if}
</section>
