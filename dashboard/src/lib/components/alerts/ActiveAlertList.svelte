<!--
  ActiveAlertList — firing and pending alerts with acknowledge and snooze.

  <ActiveAlertList alerts={active} canRespond={can('apps:control')}
    onchanged={refresh} onopen={(episodeId) => …} />
-->
<script lang="ts">
	import { acknowledgeAlert, snoozeAlert, unsnoozeAlert, type ActiveAlert } from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import {
		SEVERITY_LABEL,
		SNOOZE_OPTIONS,
		formatDuration,
		secondsBetween,
		severityTone,
		targetHref,
		valueVsThreshold
	} from '$lib/alerts';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import CheckIcon from '~icons/ri/check-line';
	import SnoozeIcon from '~icons/ri/notification-snooze-line';
	import WarnIcon from '~icons/ri/error-warning-line';
	import { cn } from '$lib/utils';

	let {
		alerts,
		canRespond = false,
		showRule = true,
		onchanged,
		onopen
	}: {
		alerts: ActiveAlert[];
		canRespond?: boolean;
		/** Show the rule name (off on the rule's own page) */
		showRule?: boolean;
		onchanged?: () => unknown;
		onopen?: (episodeId: string) => void;
	} = $props();

	let now = $state(Date.now());
	// Keep "firing for …" current (paused while the tab is hidden).
	$effect(() => pollWhileVisible(() => (now = Date.now()), 30_000, { immediate: false }));

	let openSnooze = $state<string | null>(null);

	const key = (a: ActiveAlert) => `${a.rule_id}|${a.target}`;

	const ack = useAction(
		async (a: ActiveAlert) => {
			if (a.episode_id) await acknowledgeAlert(a.episode_id);
			await onchanged?.();
		},
		{ key }
	);

	const snooze = useAction(
		async (a: ActiveAlert, opt: { minutes?: number; untilResolved?: boolean }) => {
			openSnooze = null;
			await snoozeAlert(a.rule_id, {
				target: a.target,
				minutes: opt.minutes,
				until_resolved: opt.untilResolved ?? false
			});
			await onchanged?.();
		},
		{ key: (a) => key(a) }
	);

	const unsnooze = useAction(
		async (a: ActiveAlert) => {
			await unsnoozeAlert(a.rule_id, a.target);
			await onchanged?.();
		},
		{ key }
	);

	function snoozeText(a: ActiveAlert): string | null {
		if (a.snoozed_until_resolved) return 'Snoozed until resolved';
		if (!a.snoozed_until) return null;
		const until = new Date(a.snoozed_until);
		const sameDay = until.toDateString() === new Date(now).toDateString();
		const time = until.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
		return `Snoozed until ${sameDay ? time : `${until.toLocaleDateString()} ${time}`}`;
	}

	let error = $derived(ack.error || snooze.error || unsnooze.error);
</script>

<InlineError
	message={error}
	ondismiss={() => {
		ack.clear();
		snooze.clear();
		unsnooze.clear();
	}}
/>

<ul class="divide-y rounded-sm border bg-card" aria-label="Active alerts">
	{#each alerts as a (key(a))}
		{@const href = targetHref(a.target)}
		{@const snoozed = snoozeText(a)}
		{@const busy = ack.isPending(key(a)) || snooze.isPending(key(a)) || unsnooze.isPending(key(a))}
		<li
			class={cn(
				'flex flex-col gap-3 px-4 py-3 sm:flex-row sm:items-center sm:justify-between',
				a.state === 'pending' && 'bg-muted/30'
			)}
		>
			<div class="min-w-0 space-y-1">
				<div class="flex flex-wrap items-center gap-2">
					{#if a.state === 'firing'}
						<StatusBadge
							status={a.severity}
							tone={severityTone(a.severity)}
							label={SEVERITY_LABEL[a.severity]}
							size="sm"
						/>
					{:else}
						<StatusBadge status="pending" label="Pending" size="sm" />
					{/if}
					<span class="font-medium break-words">
						{#if showRule}
							<a href={`/alerts/${a.rule_id}`} class="hover:underline">{a.rule_name}</a>
							<span class="text-muted-foreground">·</span>
						{/if}
						{#if href}
							<a {href} class="hover:underline">{a.target_label}</a>
						{:else}
							{a.target_label}
						{/if}
					</span>
				</div>
				<p class="text-sm text-muted-foreground">
					{valueVsThreshold(a)}
					·
					{#if a.state === 'firing' && a.fired_at}
						firing for {formatDuration(secondsBetween(a.fired_at, now))}
					{:else}
						true for {formatDuration(secondsBetween(a.since, now))}, waiting before it fires
					{/if}
				</p>
				{#if a.acknowledged_at || snoozed || a.failed_deliveries > 0}
					<p class="flex flex-wrap gap-x-3 gap-y-1 text-xs">
						{#if a.acknowledged_at}
							<span class="text-muted-foreground">
								Acknowledged{a.acknowledged_by ? ` by ${a.acknowledged_by}` : ''}
							</span>
						{/if}
						{#if snoozed}
							<span class="text-muted-foreground">{snoozed}</span>
						{/if}
						{#if a.failed_deliveries > 0}
							<span class="inline-flex items-center gap-1 text-destructive">
								<WarnIcon class="size-3.5" aria-hidden="true" />
								{a.failed_deliveries} notification{a.failed_deliveries === 1 ? '' : 's'} couldn't be delivered
							</span>
						{/if}
					</p>
				{/if}
			</div>
			<div class="flex shrink-0 flex-wrap items-center gap-2">
				{#if a.episode_id && onopen}
					<Button variant="ghost" size="sm" onclick={() => onopen?.(a.episode_id ?? '')}>
						Details
					</Button>
				{/if}
				{#if canRespond}
					{#if a.state === 'firing' && !a.acknowledged_at}
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							aria-busy={ack.isPending(key(a))}
							onclick={() => ack.run(a)}
						>
							<CheckIcon class="size-4" aria-hidden="true" />
							{ack.isPending(key(a)) ? 'Acknowledging…' : 'Acknowledge'}
						</Button>
					{/if}
					{#if snoozed}
						<Button
							variant="outline"
							size="sm"
							disabled={busy}
							aria-busy={unsnooze.isPending(key(a))}
							onclick={() => unsnooze.run(a)}
						>
							{unsnooze.isPending(key(a)) ? 'Unsnoozing…' : 'Unsnooze'}
						</Button>
					{:else}
						<Popover.Root
							bind:open={() => openSnooze === key(a), (o) => (openSnooze = o ? key(a) : null)}
						>
							<Popover.Trigger>
								{#snippet child({ props })}
									<Button
										{...props}
										variant="outline"
										size="sm"
										disabled={busy}
										aria-busy={snooze.isPending(key(a))}
									>
										<SnoozeIcon class="size-4" aria-hidden="true" />
										{snooze.isPending(key(a)) ? 'Snoozing…' : 'Snooze'}
									</Button>
								{/snippet}
							</Popover.Trigger>
							<Popover.Content class="w-48 p-1" align="end">
								<p class="px-2 py-1.5 text-xs text-muted-foreground">Mute notifications for</p>
								{#each SNOOZE_OPTIONS as opt (opt.label)}
									<button
										type="button"
										class="w-full rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
										onclick={() => snooze.run(a, opt)}
									>
										{opt.label}
									</button>
								{/each}
							</Popover.Content>
						</Popover.Root>
					{/if}
				{/if}
			</div>
		</li>
	{/each}
</ul>
