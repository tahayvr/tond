<script lang="ts">
	import { page } from '$app/state';
	import { goto, invalidateAll } from '$app/navigation';
	import { can } from '$lib/auth.svelte';
	import {
		deleteAlertRule,
		snoozeAlert,
		testAlertRule,
		unsnoozeAlert,
		updateAlertRule,
		type ActiveAlert,
		type AlertChannelTestResult,
		type AlertKindInfo,
		type AlertRule,
		type AlertRuleInput,
		type App,
		type Integration,
		type QuietHours
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import {
		CHANNEL_KIND_LABEL,
		FOR_OPTIONS,
		REPEAT_OPTIONS,
		SEVERITY_LABEL,
		conditionSummary,
		kindLabel,
		optionLabel,
		severityTone
	} from '$lib/alerts';
	import { confirmAction } from '$lib/confirm.svelte';
	import { ALERT_TABS, allowedTabs, tabFromUrl, tabHref, type AlertTab } from '$lib/tabs';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import LinkTabs from '$lib/components/nav/link-tabs.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ActiveAlertList from '$lib/components/alerts/ActiveAlertList.svelte';
	import AlertHistory from '$lib/components/alerts/AlertHistory.svelte';
	import AlertEpisodeDialog from '$lib/components/alerts/AlertEpisodeDialog.svelte';
	import AlertRuleForm from '$lib/components/alerts/AlertRuleForm.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import AllClear from '~icons/ri/checkbox-circle-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let rule = $derived(data.rule as AlertRule);
	let active = $derived(data.active as ActiveAlert[]);
	let kinds = $derived(data.kinds as AlertKindInfo[]);
	let apps = $derived(data.apps as App[]);
	let channels = $derived(data.integrations as Integration[]);
	let quietHours = $derived((data.quietHours ?? undefined) as QuietHours | undefined);
	let tab = $derived<AlertTab>(tabFromUrl(page.url, ALERT_TABS, 'overview'));
	let episodeId = $state<string | null>(null);
	let historyKey = $state(0);
	let muteOpen = $state(false);
	let saved = $state('');

	let mutedUntil = $derived(
		rule.snoozed_until && new Date(rule.snoozed_until).getTime() > Date.now()
			? new Date(rule.snoozed_until)
			: null
	);
	let linked = $derived(channels.filter((c) => rule.integration_ids.includes(c.id)));

	let testResults = $state<AlertChannelTestResult[]>([]);
	const sendTest = useAction(async () => {
		testResults = await testAlertRule(rule.id);
	});
	let testSummary = $derived(
		testResults.length > 0 && testResults.every((r) => r.ok)
			? `Test sent to ${testResults.length} channel${testResults.length === 1 ? '' : 's'}. Check that it arrived.`
			: ''
	);

	const toggleEnabled = useAction(() => updateAlertRule(rule.id, { enabled: !rule.enabled }), {
		invalidate: true
	});
	const mute = useAction(
		async (minutes: number | null) => {
			muteOpen = false;
			if (minutes === null) await unsnoozeAlert(rule.id);
			else await snoozeAlert(rule.id, { minutes });
		},
		{ invalidate: true }
	);

	async function refresh() {
		await invalidateAll();
		historyKey += 1;
	}

	async function save(input: AlertRuleInput) {
		await updateAlertRule(rule.id, {
			name: input.name,
			threshold: input.threshold,
			window_secs: input.window_secs,
			for_secs: input.for_secs,
			severity: input.severity,
			repeat_secs: input.repeat_secs,
			integration_ids: input.integration_ids
		});
		await invalidateAll();
		saved = 'Rule saved.';
	}

	function handleDelete() {
		void confirmAction({
			title: `Delete ${rule.name}?`,
			description:
				"You won't be notified about this condition any more. Its alert history is kept for 90 days; your channels stay.",
			confirmLabel: 'Delete rule',
			pendingLabel: 'Deleting…',
			onconfirm: async () => {
				await deleteAlertRule(rule.id);
				await goto('/alerts?tab=rules', { invalidateAll: true });
			}
		});
	}

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<Breadcrumb items={[{ label: 'Alerts', href: '/alerts?tab=rules' }, { label: rule.name }]} />

	<PageHeader title={rule.name} section={ALERT_TABS.find((t) => t.id === tab)?.label}>
		{#snippet meta()}
			{#if !rule.enabled}
				<StatusBadge status="paused" />
			{:else if rule.firing > 0}
				<StatusBadge status="firing" label={rule.firing > 1 ? `${rule.firing} firing` : 'Firing'} />
			{:else if mutedUntil}
				<StatusBadge status="paused" label="Muted" />
			{:else}
				<StatusBadge status="success" label="Watching" />
			{/if}
			<StatusBadge
				status={rule.severity}
				tone={severityTone(rule.severity)}
				label={SEVERITY_LABEL[rule.severity]}
			/>
			<span class="text-sm text-muted-foreground">{conditionSummary(rule)}</span>
		{/snippet}
		{#snippet actions()}
			{#if can('settings:write')}
				<Button
					variant="outline"
					size="sm"
					onclick={() => sendTest.run()}
					disabled={sendTest.pending}
					aria-busy={sendTest.pending}
				>
					{sendTest.pending ? 'Sending test…' : 'Send test'}
				</Button>
				<Button
					variant="outline"
					size="sm"
					onclick={() => toggleEnabled.run()}
					disabled={toggleEnabled.pending}
					aria-busy={toggleEnabled.pending}
				>
					{toggleEnabled.pending ? 'Saving…' : rule.enabled ? 'Pause' : 'Turn on'}
				</Button>
			{/if}
			{#if can('apps:control') && rule.enabled}
				{#if mutedUntil}
					<Button
						variant="outline"
						size="sm"
						onclick={() => mute.run(null)}
						disabled={mute.pending}
						aria-busy={mute.pending}
					>
						{mute.pending ? 'Unmuting…' : 'Unmute'}
					</Button>
				{:else}
					<Popover.Root bind:open={muteOpen}>
						<Popover.Trigger>
							{#snippet child({ props })}
								<Button {...props} variant="outline" size="sm" disabled={mute.pending}>
									{mute.pending ? 'Muting…' : 'Mute'}
								</Button>
							{/snippet}
						</Popover.Trigger>
						<Popover.Content class="w-48 p-1" align="end">
							<p class="px-2 py-1.5 text-xs text-muted-foreground">Mute the whole rule for</p>
							{#each [{ l: '1 hour', m: 60 }, { l: '8 hours', m: 480 }, { l: '24 hours', m: 1440 }, { l: '7 days', m: 10_080 }] as o (o.m)}
								<button
									type="button"
									class="w-full rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
									onclick={() => mute.run(o.m)}
								>
									{o.l}
								</button>
							{/each}
						</Popover.Content>
					</Popover.Root>
				{/if}
			{/if}
		{/snippet}
	</PageHeader>

	<InlineError message={toggleEnabled.error} ondismiss={toggleEnabled.clear} />
	<InlineError message={mute.error} ondismiss={mute.clear} />
	<InlineError title="Test not sent" message={sendTest.error} ondismiss={sendTest.clear} />
	<SuccessAlert message={testSummary} ondismiss={() => (testResults = [])} />
	{#if testResults.some((r) => !r.ok)}
		<InlineError
			title="Some channels didn't receive the test"
			message={testResults
				.map(
					(r) =>
						`${r.ok ? '✓' : '✕'} ${r.name} (${CHANNEL_KIND_LABEL[r.kind as keyof typeof CHANNEL_KIND_LABEL] ?? r.kind})${r.error ? `: ${r.error}` : ''}`
				)
				.join('\n')}
			ondismiss={() => (testResults = [])}
		/>
	{/if}
	{#if mutedUntil}
		<p class="text-sm text-muted-foreground">
			Muted until {mutedUntil.toLocaleString()}: alerts still fire and show here, but nobody is
			notified.
		</p>
	{/if}

	<LinkTabs
		label="Alert rule sections"
		tabs={allowedTabs(ALERT_TABS, can)}
		active={tab}
		href={(id) => tabHref(page.url.pathname, id, 'overview')}
	/>

	{#if tab === 'overview'}
		<section class="space-y-3" aria-labelledby="now-heading">
			<h2 id="now-heading" class="text-lg font-semibold">Right now</h2>
			{#if active.length}
				<ActiveAlertList
					alerts={active}
					showRule={false}
					canRespond={can('apps:control')}
					onchanged={refresh}
					onopen={(id) => (episodeId = id)}
				/>
			{:else}
				<div class="flex items-center gap-3 rounded-sm border bg-card px-4 py-4">
					<AllClear class="size-5 shrink-0 text-success" aria-hidden="true" />
					<p class="text-sm">
						{rule.enabled ? 'Nothing is firing for this rule.' : 'This rule is paused.'}
					</p>
				</div>
			{/if}
		</section>

		<section class="grid gap-4 sm:grid-cols-2" aria-label="Rule settings">
			<dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 rounded-sm border bg-card p-4 text-sm">
				<dt class="text-muted-foreground">Watches</dt>
				<dd>{kindLabel(rule.kind)}</dd>
				<dt class="text-muted-foreground">Where</dt>
				<dd>
					{#if rule.kind === 'disk_usage' || rule.kind === 'disk_forecast'}Server
					{:else if rule.kind === 'cert_expiry'}Every certificate
					{:else if rule.kind === 'backup_failed'}Backups
					{:else if rule.kind === 'cron_failed'}Every cron job
					{:else if rule.app_id}<a
							href={`/apps/${rule.app_id}`}
							class="underline underline-offset-2">{rule.app_name ?? 'One app'}</a
						>
					{:else}Every app{/if}
				</dd>
				<dt class="text-muted-foreground">Fires after</dt>
				<dd>{optionLabel(FOR_OPTIONS, rule.for_secs)}</dd>
				<dt class="text-muted-foreground">Reminders</dt>
				<dd>{optionLabel(REPEAT_OPTIONS, rule.repeat_secs)}</dd>
			</dl>
			<div class="space-y-2 rounded-sm border bg-card p-4 text-sm">
				<h3 class="text-muted-foreground">Notifies</h3>
				{#if linked.length}
					<ul class="space-y-1">
						{#each linked as c (c.id)}
							<li class="flex flex-wrap items-center justify-between gap-2">
								<span>{c.name}</span>
								<span class="text-xs text-muted-foreground">
									{CHANNEL_KIND_LABEL[c.kind]}{c.enabled ? '' : ' · switched off'}
								</span>
								{#if c.last_error}
									<span class="w-full text-xs break-words text-destructive">
										Last delivery failed: {c.last_error}
									</span>
								{/if}
							</li>
						{/each}
					</ul>
				{:else}
					<p>
						{rule.integration_ids.length} channel{rule.integration_ids.length === 1 ? '' : 's'}
					</p>
				{/if}
			</div>
		</section>

		<section class="space-y-3" aria-labelledby="rule-history-heading">
			<h2 id="rule-history-heading" class="text-lg font-semibold">History</h2>
			<AlertHistory ruleId={rule.id} refreshKey={historyKey} onopen={(id) => (episodeId = id)} />
		</section>
	{:else if tab === 'settings' && can('settings:write')}
		<SuccessAlert message={saved} ondismiss={() => (saved = '')} timeout={4000} />
		{#key rule.updated_at}
			<AlertRuleForm
				{kinds}
				{apps}
				{channels}
				{rule}
				globalQuiet={quietHours}
				submitLabel="Save rule"
				pendingLabel="Saving…"
				onsubmit={save}
			/>
		{/key}

		<section class="max-w-2xl space-y-3 rounded-sm border border-destructive/30 p-4">
			<h2 class="text-sm font-semibold text-destructive">Danger zone</h2>
			<p class="text-sm text-muted-foreground">Delete this rule. Its history stays for 90 days.</p>
			<Button
				variant="outline"
				size="sm"
				class="border-destructive/50 text-destructive hover:border-destructive hover:text-destructive"
				onclick={handleDelete}
			>
				Delete rule…
			</Button>
		</section>
	{/if}
</div>

<AlertEpisodeDialog bind:episodeId />
