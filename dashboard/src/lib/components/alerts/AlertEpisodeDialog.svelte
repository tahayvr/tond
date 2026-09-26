<!--
  AlertEpisodeDialog — one alert from fired to resolved, with every
  notification sent (or not) for it.

  <AlertEpisodeDialog bind:episodeId />   (null = closed)
-->
<script lang="ts">
	import { getAlertEpisode, type AlertDelivery, type AlertEpisodeDetail } from '$lib/api';
	import { errorMessage } from '$lib/action.svelte';
	import {
		CHANNEL_KIND_LABEL,
		SEVERITY_LABEL,
		formatAlertValue,
		formatDuration,
		kindLabel,
		secondsBetween,
		severityTone,
		targetHref,
		valueVsThreshold
	} from '$lib/alerts';
	import { formatRelativeFuture } from '$lib/time';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';

	let { episodeId = $bindable(null) }: { episodeId: string | null } = $props();

	let detail = $state<AlertEpisodeDetail | null>(null);
	let error = $state('');

	$effect(() => {
		const id = episodeId;
		detail = null;
		error = '';
		if (!id) return;
		let cancelled = false;
		getAlertEpisode(id)
			.then((d) => !cancelled && (detail = d))
			.catch((e) => !cancelled && (error = errorMessage(e, "This alert couldn't be loaded.")));
		return () => {
			cancelled = true;
		};
	});

	function when(iso: string | null): string {
		return iso ? new Date(iso).toLocaleString() : '—';
	}

	const EVENT_LABEL: Record<AlertDelivery['event'], string> = {
		firing: 'Fired',
		repeat: 'Reminder',
		resolved: 'Resolved'
	};

	function deliveryStatus(d: AlertDelivery): { status: string; label: string } {
		switch (d.status) {
			case 'sent':
				return { status: 'success', label: 'Sent' };
			case 'failed':
				return { status: 'failed', label: 'Failed' };
			case 'skipped':
				return { status: 'stopped', label: 'Not sent' };
			default:
				return d.deferred && d.attempts === 0
					? { status: 'paused', label: 'Quiet hours' }
					: { status: 'pending', label: d.attempts > 0 ? 'Retrying' : 'Sending' };
		}
	}
</script>

<Dialog.Root
	open={episodeId !== null}
	onOpenChange={(open) => {
		if (!open) episodeId = null;
	}}
>
	<Dialog.Content class="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
		{#if detail}
			{@const ep = detail.episode}
			{@const href = targetHref(ep.target)}
			<Dialog.Header>
				<Dialog.Title class="break-words">{ep.rule_name} · {ep.target_label}</Dialog.Title>
				<Dialog.Description>{kindLabel(ep.kind)}</Dialog.Description>
			</Dialog.Header>
			<div class="space-y-5 text-sm">
				<div class="flex flex-wrap items-center gap-2">
					{#if ep.state === 'firing'}
						<StatusBadge status="firing" label="Firing" />
					{:else}
						<StatusBadge status="success" label="Resolved" />
					{/if}
					<StatusBadge
						status={ep.severity}
						tone={severityTone(ep.severity)}
						label={SEVERITY_LABEL[ep.severity]}
					/>
				</div>
				<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5">
					<dt class="text-muted-foreground">Condition</dt>
					<dd>{valueVsThreshold(ep)}</dd>
					{#if ep.comparison !== 'none' && ep.last_value !== null && ep.last_value !== ep.value}
						<dt class="text-muted-foreground">Latest value</dt>
						<dd>{formatAlertValue(ep.last_value, ep.unit)}</dd>
					{/if}
					<dt class="text-muted-foreground">Target</dt>
					<dd>
						{#if href}<a {href} class="underline underline-offset-2">{ep.target_label}</a>
						{:else}{ep.target_label}{/if}
					</dd>
					<dt class="text-muted-foreground">Fired</dt>
					<dd>{when(ep.fired_at)}</dd>
					<dt class="text-muted-foreground">{ep.resolved_at ? 'Resolved' : 'Firing for'}</dt>
					<dd>
						{#if ep.resolved_at}
							{when(ep.resolved_at)} (after {formatDuration(
								secondsBetween(ep.fired_at, ep.resolved_at)
							)})
						{:else}
							{formatDuration(secondsBetween(ep.fired_at))}
						{/if}
					</dd>
					{#if ep.acknowledged_at}
						<dt class="text-muted-foreground">Acknowledged</dt>
						<dd>
							{when(ep.acknowledged_at)}{ep.acknowledged_by ? ` by ${ep.acknowledged_by}` : ''}
						</dd>
					{/if}
				</dl>

				<section class="space-y-2">
					<h3 class="text-sm font-semibold">Notifications</h3>
					{#if detail.deliveries.length === 0}
						<p class="text-muted-foreground">
							Nothing was sent: the alert was snoozed or its rule was paused.
						</p>
					{:else}
						<ul class="divide-y rounded-sm border">
							{#each detail.deliveries as d (d.id)}
								{@const s = deliveryStatus(d)}
								<li class="space-y-1 px-3 py-2">
									<div class="flex flex-wrap items-center justify-between gap-2">
										<span>
											<span class="font-medium">{EVENT_LABEL[d.event]}</span>
											→ {d.integration_name}
											<span class="text-muted-foreground">
												({CHANNEL_KIND_LABEL[d.integration_kind] ?? d.integration_kind})
											</span>
										</span>
										<StatusBadge status={s.status} label={s.label} size="sm" />
									</div>
									<p class="text-xs text-muted-foreground">
										{#if d.sent_at}
											Sent {when(d.sent_at)}{d.attempts > 1 ? ` after ${d.attempts} attempts` : ''}
										{:else if d.status === 'pending' && d.next_attempt_at}
											{d.deferred && d.attempts === 0
												? 'Held by quiet hours, goes out'
												: 'Next try'}
											{formatRelativeFuture(d.next_attempt_at)}
											{d.attempts > 0
												? ` · ${d.attempts} attempt${d.attempts === 1 ? '' : 's'} so far`
												: ''}
										{:else if d.status === 'failed'}
											Gave up after {d.attempts} attempts
										{/if}
									</p>
									{#if d.last_error}
										<p
											class={d.status === 'skipped'
												? 'text-xs text-muted-foreground'
												: 'text-xs break-words text-destructive'}
										>
											{d.last_error}
										</p>
									{/if}
								</li>
							{/each}
						</ul>
						{#if detail.deliveries.some((d) => d.status === 'failed')}
							<p class="text-xs text-muted-foreground">
								Check the channel's settings on the
								<a href="/integrations" class="underline underline-offset-2">Integrations</a>
								page and send it a test.
							</p>
						{/if}
					{/if}
				</section>
			</div>
		{:else if error}
			<Dialog.Header>
				<Dialog.Title>Alert</Dialog.Title>
			</Dialog.Header>
			<InlineError message={error} />
		{:else}
			<Dialog.Header>
				<Dialog.Title>Loading alert…</Dialog.Title>
			</Dialog.Header>
			<div class="space-y-2">
				<Skeleton class="h-5 w-2/3" />
				<Skeleton class="h-20 w-full" />
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
