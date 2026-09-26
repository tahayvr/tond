<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { can } from '$lib/auth.svelte';
	import {
		getAlertSettings,
		listActiveAlerts,
		updateAlertSettings,
		type ActiveAlert,
		type AlertRule,
		type App,
		type QuietHours
	} from '$lib/api';
	import { errorMessage, useAction } from '$lib/action.svelte';
	import { localTimeZone } from '$lib/alerts';
	import { pollWhileVisible } from '$lib/polling';
	import { ALERTS_PAGE_TABS, tabFromUrl, tabHref, type AlertsPageTab } from '$lib/tabs';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import LinkTabs from '$lib/components/nav/link-tabs.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import AlertRuleGrid from '$lib/components/cards/alert-rule-card/alert-rule-grid.svelte';
	import ActiveAlertList from '$lib/components/alerts/ActiveAlertList.svelte';
	import AlertHistory from '$lib/components/alerts/AlertHistory.svelte';
	import AlertEpisodeDialog from '$lib/components/alerts/AlertEpisodeDialog.svelte';
	import QuietHoursFields from '$lib/components/alerts/QuietHoursFields.svelte';
	import Bell from '~icons/ri/notification-3-line';
	import AllClear from '~icons/ri/checkbox-circle-line';
	import Plus from '~icons/ri/add-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let rules = $derived(data.rules as AlertRule[]);
	let apps = $derived(data.apps as App[]);
	// Live alerts start from the page data and refresh while the tab is visible.
	let live = $state<ActiveAlert[] | null>(null);
	let active = $derived(live ?? (data.active as ActiveAlert[]));
	let firing = $derived(active.filter((a) => a.state === 'firing'));
	let pending = $derived(active.filter((a) => a.state === 'pending'));
	let failedDeliveries = $derived(firing.reduce((n, a) => n + a.failed_deliveries, 0));
	let enabledRules = $derived(rules.filter((r) => r.enabled).length);
	let historyKey = $state(0);

	let tab = $derived<AlertsPageTab>(tabFromUrl(page.url, ALERTS_PAGE_TABS, 'alerts'));
	// `?alert=<id>` (links in notifications) opens that alert's details.
	let episodeId = $derived<string | null>(page.url.searchParams.get('alert'));

	async function refresh() {
		const next = await listActiveAlerts();
		const before = active.map((a) => `${a.episode_id}|${a.state}`).join();
		live = next;
		if (next.map((a) => `${a.episode_id}|${a.state}`).join() !== before) historyKey += 1;
	}

	$effect(() => pollWhileVisible(refresh, 30_000, { immediate: false }));

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});

	function closeEpisode(id: string | null) {
		episodeId = id;
		if (id === null && page.url.searchParams.has('alert')) {
			void goto(tabHref(page.url.pathname, tab, 'alerts'), {
				replaceState: true,
				noScroll: true,
				keepFocus: true
			});
		}
	}

	// ── Quiet hours (global) ──
	let quiet = $state<QuietHours | null>(null);
	let quietError = $state('');
	$effect(() => {
		if (tab !== 'settings' || quiet) return;
		getAlertSettings()
			.then((s) => {
				quiet = s.quiet_hours.enabled
					? s.quiet_hours
					: {
							...s.quiet_hours,
							timezone: s.quiet_hours.timezone === 'UTC' ? localTimeZone() : s.quiet_hours.timezone
						};
			})
			.catch((e) => (quietError = errorMessage(e, "Quiet hours couldn't be loaded.")));
	});
	const saveQuiet = useAction(
		async () => {
			if (quiet) quiet = (await updateAlertSettings({ quiet_hours: quiet })).quiet_hours;
		},
		{ success: 'Quiet hours saved.' }
	);
</script>

<div class="space-y-6">
	<PageHeader
		title="Alerts"
		section={tab === 'alerts' ? null : ALERTS_PAGE_TABS.find((t) => t.id === tab)?.label}
		description={rules.length
			? `${firing.length ? `${firing.length} firing · ` : ''}${enabledRules} of ${rules.length} rules on`
			: undefined}
	>
		{#snippet actions()}
			{#if can('settings:write')}
				<Button href="/alerts/new">
					<Plus class="size-4" aria-hidden="true" />
					New rule
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<LinkTabs
		label="Alert sections"
		tabs={ALERTS_PAGE_TABS}
		active={tab}
		href={(id) => tabHref(page.url.pathname, id, 'alerts')}
	/>

	{#if tab === 'alerts'}
		{#if failedDeliveries > 0}
			<InlineError
				title="Some notifications couldn't be delivered"
				message={`${failedDeliveries} notification${failedDeliveries === 1 ? '' : 's'} about firing alerts failed after several tries. Open an alert's details to see why, then fix the channel on the Integrations page.`}
			/>
		{/if}

		<section class="space-y-3" aria-labelledby="firing-heading">
			<h2 id="firing-heading" class="text-lg font-semibold">
				Firing now {#if firing.length}<span class="text-muted-foreground">({firing.length})</span
					>{/if}
			</h2>
			{#if firing.length}
				<ActiveAlertList
					alerts={firing}
					canRespond={can('apps:control')}
					onchanged={refresh}
					onopen={(id) => (episodeId = id)}
				/>
			{:else}
				<div class="flex items-center gap-3 rounded-sm border bg-card px-4 py-5">
					<AllClear class="size-6 shrink-0 text-success" aria-hidden="true" />
					<div>
						<p class="font-medium">All clear</p>
						<p class="text-sm text-muted-foreground">
							{#if rules.length === 0}
								No rules yet. Create one to hear about crashes, failed deploys or a filling disk.
							{:else}
								Nothing is firing. {enabledRules} rule{enabledRules === 1 ? ' is' : 's are'} watching.
							{/if}
						</p>
					</div>
				</div>
			{/if}
		</section>

		{#if pending.length}
			<section class="space-y-3" aria-labelledby="pending-heading">
				<div>
					<h2 id="pending-heading" class="text-lg font-semibold">
						Pending <span class="text-muted-foreground">({pending.length})</span>
					</h2>
					<p class="text-sm text-muted-foreground">
						The condition is true, but hasn't lasted long enough to fire yet.
					</p>
				</div>
				<ActiveAlertList alerts={pending} canRespond={can('apps:control')} onchanged={refresh} />
			</section>
		{/if}

		<section class="space-y-3" aria-labelledby="history-heading">
			<h2 id="history-heading" class="text-lg font-semibold">History</h2>
			<AlertHistory {rules} {apps} refreshKey={historyKey} onopen={(id) => (episodeId = id)} />
		</section>
	{:else if tab === 'rules'}
		{#if rules.length === 0}
			<div
				class="flex flex-col items-center justify-center rounded-sm border bg-card px-4 py-16 text-center"
			>
				<Bell aria-hidden="true" class="mb-4 size-12 text-muted-foreground" />
				<p class="font-medium">No alert rules yet</p>
				<p class="mt-2 max-w-md text-sm text-muted-foreground">
					Get a push, chat message or email when an app crashes, a deploy or backup fails, a
					certificate is about to expire or the disk is filling up.
				</p>
				{#if can('settings:write')}
					<Button class="mt-4" href="/alerts/new">
						<Plus class="size-4" aria-hidden="true" />
						New rule
					</Button>
				{/if}
			</div>
		{:else}
			<AlertRuleGrid {rules} />
		{/if}
	{:else if tab === 'settings'}
		<section class="max-w-2xl space-y-4" aria-labelledby="quiet-heading">
			<div>
				<h2 id="quiet-heading" class="text-lg font-semibold">Quiet hours</h2>
				<p class="text-sm text-muted-foreground">
					A daily window when notifications wait. Channels follow this unless you give them their
					own quiet hours on the Integrations page.
				</p>
			</div>
			<InlineError message={quietError} />
			<InlineError message={saveQuiet.error} ondismiss={saveQuiet.clear} />
			<SuccessAlert message={saveQuiet.success} ondismiss={saveQuiet.clear} timeout={4000} />
			{#if quiet}
				<form
					class="space-y-4 rounded-sm border bg-card p-4"
					onsubmit={(e) => {
						e.preventDefault();
						saveQuiet.run();
					}}
				>
					<div class="flex items-center gap-2">
						<Checkbox
							id="quiet-enabled"
							checked={quiet.enabled}
							disabled={!can('settings:write')}
							onCheckedChange={(c) => quiet && (quiet.enabled = c === true)}
						/>
						<Label for="quiet-enabled">Hold back notifications during quiet hours</Label>
					</div>
					<QuietHoursFields
						idPrefix="global-quiet"
						bind:value={quiet}
						disabled={!quiet.enabled || !can('settings:write')}
					/>
					{#if can('settings:write')}
						<Button type="submit" disabled={saveQuiet.pending} aria-busy={saveQuiet.pending}>
							{saveQuiet.pending ? 'Saving…' : 'Save quiet hours'}
						</Button>
					{/if}
				</form>
			{:else if !quietError}
				<Skeleton class="h-40 w-full" />
			{/if}
		</section>
	{/if}
</div>

<AlertEpisodeDialog bind:episodeId={() => episodeId, closeEpisode} />
