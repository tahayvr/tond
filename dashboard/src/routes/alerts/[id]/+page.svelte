<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { page } from '$app/state';
	import {
		updateAlertRule,
		deleteAlertRule,
		testAlertRule,
		ALERT_GROUPS,
		type AlertChannelTestResult,
		type AlertRule,
		type AlertMetric,
		type AlertEvent,
		type App
	} from '$lib/api';
	import { SvelteSet } from 'svelte/reactivity';
	import { goto } from '$app/navigation';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import LinkTabs from '$lib/components/nav/link-tabs.svelte';
	import ConfirmDialog from '$lib/components/dialogs/confirm-dialog.svelte';
	import { useAction } from '$lib/action.svelte';
	import { ALERT_TABS, tabFromUrl, tabHref, type AlertTab } from '$lib/tabs';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import IntegrationSelector from '$lib/components/integrations/integration-selector.svelte';
	import Bell from '~icons/ri/notification-3-line';
	import Plug from '~icons/ri/plug-line';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();

	let rule = $derived(data.rule as AlertRule | null);
	let error = $state('');
	let showDeleteDialog = $state(false);
	let tab = $derived<AlertTab>(tabFromUrl(page.url, ALERT_TABS, 'overview'));
	let testResults = $state<AlertChannelTestResult[]>([]);
	const sendTest = useAction(async () => {
		testResults = await testAlertRule(ruleId);
	});
	let testSummary = $derived(
		testResults.length === 0
			? ''
			: testResults.every((r) => r.ok)
				? `Test sent to ${testResults.length} channel${testResults.length === 1 ? '' : 's'}. Check that it arrived.`
				: ''
	);

	// Apps list for resolving app names
	let apps = $derived(data.apps as App[]);

	// Settings form state — seeded from rule on load
	let settingsName = $state('');
	let settingsGroupId = $state(ALERT_GROUPS[0].id);
	let settingsMetric = $state<AlertMetric | ''>('');
	let settingsThreshold = $state(80);
	let settingsEvents = new SvelteSet<AlertEvent>();
	let settingsWebhookUrl = $state('');
	let settingsSaving = $state(false);
	let settingsError = $state('');
	let settingsSuccess = $state('');

	// Derived: current settings group
	let settingsGroup = $derived(
		ALERT_GROUPS.find((g) => g.id === settingsGroupId) ?? ALERT_GROUPS[0]
	);
	let settingsHasMetrics = $derived(settingsGroup.metrics.length > 0);
	let settingsHasEvents = $derived(settingsGroup.events.length > 0);
	let settingsCurrentMetric = $derived(
		settingsGroup.metrics.find((m) => m.value === settingsMetric) ?? settingsGroup.metrics[0]
	);

	let ruleId = $derived(page.params.id || params.id);

	/** Infer the group ID for a rule based on its metric and events */
	function inferGroupId(r: AlertRule): string {
		if (r.metric) {
			const group = ALERT_GROUPS.find((g) => g.metrics.some((m) => m.value === r.metric));
			if (group) return group.id;
		}
		if (r.events.length > 0) {
			const group = ALERT_GROUPS.find((g) => g.events.some((e) => r.events.includes(e.value)));
			if (group) return group.id;
		}
		return ALERT_GROUPS[0].id;
	}

	function metricLabel(metric: string | null): string {
		if (!metric) return '';
		for (const group of ALERT_GROUPS) {
			const m = group.metrics.find((x) => x.value === metric);
			if (m) return m.label;
		}
		return metric.toUpperCase();
	}

	function thresholdUnit(metric: string | null): string {
		return metric === 'app_down' ? 'min' : '%';
	}

	function formatDate(dt: string | null): string {
		if (!dt) return 'Never';
		return new Date(dt).toLocaleString();
	}

	function formatRelative(dt: string | null): string {
		if (!dt) return 'Never';
		const diff = Date.now() - new Date(dt).getTime();
		const minutes = Math.floor(diff / 60_000);
		const hours = Math.floor(diff / 3_600_000);
		const days = Math.floor(diff / 86_400_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
	}

	function eventLabel(event: AlertEvent): string {
		for (const group of ALERT_GROUPS) {
			const e = group.events.find((x) => x.value === event);
			if (e) return e.label;
		}
		return event;
	}

	const toggleEnabled = useAction(async () => {
		if (!rule) return;
		rule = await updateAlertRule(ruleId, { enabled: !rule.enabled });
	});

	function handleSettingsGroupChange(groupId: string) {
		settingsGroupId = groupId;
		const group = ALERT_GROUPS.find((g) => g.id === groupId) ?? ALERT_GROUPS[0];
		settingsMetric = group.metrics[0]?.value ?? '';
		settingsThreshold = group.metrics[0]?.placeholder ?? 80;
		settingsEvents.clear();
	}

	function handleSettingsMetricChange(metric: string) {
		settingsMetric = metric as AlertMetric;
		const m = settingsGroup.metrics.find((x) => x.value === metric);
		if (m) settingsThreshold = m.placeholder;
	}

	function toggleSettingsEvent(event: AlertEvent) {
		if (settingsEvents.has(event)) {
			settingsEvents.delete(event);
		} else {
			settingsEvents.add(event);
		}
	}

	async function handleSaveSettings() {
		settingsSaving = true;
		settingsError = '';
		settingsSuccess = '';
		try {
			rule = await updateAlertRule(ruleId, {
				name: settingsName.trim(),
				metric: settingsHasMetrics && settingsMetric ? (settingsMetric as AlertMetric) : null,
				threshold: settingsThreshold,
				webhook_url: settingsWebhookUrl.trim(),
				events: [...settingsEvents]
			});
			settingsSuccess = 'Settings saved.';
		} catch (e: any) {
			settingsError = e.message ?? 'Failed to save settings';
		} finally {
			settingsSaving = false;
		}
	}

	async function handleDelete() {
		await deleteAlertRule(ruleId);
		goto('/alerts', { invalidateAll: true });
	}

	$effect(() => {
		if (rule) {
			settingsName = rule.name;
			settingsGroupId = inferGroupId(rule);
			settingsMetric = rule.metric ?? '';
			settingsThreshold = rule.threshold;
			settingsWebhookUrl = rule.webhook_url;
			settingsEvents.clear();
			for (const ev of rule.events) settingsEvents.add(ev);
		}
	});

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});
</script>

{#if error && !rule}
	<InlineError message={error} />
{:else if rule}
	{@const current = rule}
	<div class="space-y-6">
		<!-- Breadcrumb -->
		<Breadcrumb items={[{ label: 'Alerts', href: '/alerts' }, { label: rule.name }]} />

		<PageHeader title={rule.name} section={ALERT_TABS.find((t) => t.id === tab)?.label}>
			{#snippet meta()}
				<StatusBadge status={current.enabled ? 'enabled' : 'paused'} />
				{#if current.metric}
					<span class="text-xs">
						{metricLabel(current.metric)} &gt; {current.threshold}{thresholdUnit(current.metric)}
					</span>
				{/if}
				{#if current.events.length > 0}
					<span class="text-xs">
						{current.events.length} event{current.events.length > 1 ? 's' : ''}
					</span>
				{/if}
			{/snippet}
			{#snippet actions()}
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
				>
					{toggleEnabled.pending ? 'Saving…' : current.enabled ? 'Pause' : 'Enable'}
				</Button>
			{/snippet}
		</PageHeader>

		<InlineError message={error} ondismiss={() => (error = '')} />
		<InlineError message={toggleEnabled.error} ondismiss={toggleEnabled.clear} />
		<InlineError title="Test not sent" message={sendTest.error} ondismiss={sendTest.clear} />
		<SuccessAlert message={testSummary} ondismiss={() => (testResults = [])} />
		{#if testResults.some((r) => !r.ok)}
			<div
				role="alert"
				class="space-y-1 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2.5 text-sm text-warn"
			>
				<p class="font-semibold">Some channels didn't receive the test</p>
				<ul class="space-y-0.5">
					{#each testResults as result (result.kind + result.name)}
						<li>
							{result.ok ? '✓' : '✕'}
							{result.name} ({result.kind}){result.error ? `: ${result.error}` : ''}
						</li>
					{/each}
				</ul>
			</div>
		{/if}

		<LinkTabs
			label="Alert rule sections"
			tabs={ALERT_TABS}
			active={tab}
			href={(id) => tabHref(page.url.pathname, id, 'overview')}
		/>

		<div>
			<!-- ── Overview tab ── -->
			{#if tab === 'overview'}
				<div class="space-y-4 pt-4">
					<div class="grid gap-4 sm:grid-cols-2">
						<!-- Status card -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Status
							</h4>
							<div class="space-y-1 text-sm">
								<div class="flex items-center gap-2">
									<span class="text-muted-foreground">State:</span>
									<StatusBadge status={rule.enabled ? 'enabled' : 'paused'} size="sm" />
								</div>
								<div>
									<span class="text-muted-foreground">Last fired:</span>
									{formatRelative(rule.last_fired_at)}
									{#if rule.last_fired_at}
										<span class="ml-1 font-mono text-xs text-muted-foreground">
											({formatDate(rule.last_fired_at)})
										</span>
									{/if}
								</div>
								<div>
									<span class="text-muted-foreground">Rule ID:</span>
									<span class="font-mono text-xs">{rule.id}</span>
								</div>
								<div>
									<span class="text-muted-foreground">Created:</span>
									{formatDate(rule.created_at)}
								</div>
								<div>
									<span class="text-muted-foreground">Updated:</span>
									{formatDate(rule.updated_at)}
								</div>
							</div>
						</div>

						<!-- Condition card -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Condition
							</h4>
							<div class="space-y-1 text-sm">
								{#if rule.metric}
									<div>
										<span class="text-muted-foreground">Metric:</span>
										{metricLabel(rule.metric)}
									</div>
									<div>
										<span class="text-muted-foreground">Threshold:</span>
										<span class="font-mono">{rule.threshold} {thresholdUnit(rule.metric)}</span>
									</div>
									<div>
										<span class="text-muted-foreground">Scope:</span>
										{#if rule.app_id}
											{@const appId = rule.app_id}
											{apps.find((a) => a.id === appId)?.name ?? appId.slice(0, 8)}
										{:else}
											Global — all apps
										{/if}
									</div>
								{/if}
								{#if rule.events.length > 0}
									<div>
										<span class="text-muted-foreground">Events:</span>
										<ul class="mt-0.5 list-inside list-disc space-y-0.5 text-xs">
											{#each rule.events as ev (ev)}
												<li>{eventLabel(ev)}</li>
											{/each}
										</ul>
									</div>
								{/if}
								{#if !rule.metric && rule.events.length === 0}
									<p class="text-xs text-muted-foreground">No condition configured.</p>
								{/if}
							</div>
						</div>

						<!-- Notifications card -->
						<div class="space-y-3 rounded border border-dotted p-4 sm:col-span-2">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Notifications
							</h4>
							<div class="space-y-3 text-sm">
								{#if rule.webhook_url}
									<div class="flex items-start gap-2">
										<Bell class="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
										<div class="min-w-0">
											<p class="font-mono text-xs font-medium uppercase">Webhook</p>
											<p class="truncate font-mono text-xs text-muted-foreground">
												{rule.webhook_url}
											</p>
										</div>
									</div>
								{:else}
									<p class="text-xs text-muted-foreground">No webhook configured.</p>
								{/if}

								<div class="mt-1">
									<p
										class="mb-2 flex items-center gap-1.5 font-mono text-xs font-medium text-muted-foreground uppercase"
									>
										<Plug class="h-3.5 w-3.5" />
										Notification Channels
									</p>
									<IntegrationSelector {ruleId} />
								</div>
							</div>
						</div>
					</div>
				</div>
			{/if}

			<!-- ── Settings tab ── -->
			{#if tab === 'settings'}
				<div class="space-y-6 pt-4">
					<InlineError message={settingsError} />
					<SuccessAlert message={settingsSuccess} ondismiss={() => (settingsSuccess = '')} />

					<div class="grid max-w-xl gap-5">
						<!-- Name -->
						<div class="grid gap-2">
							<Label for="s-name">Name</Label>
							<Input id="s-name" type="text" bind:value={settingsName} />
						</div>

						<!-- Category / Group -->
						<div class="grid gap-2">
							<Label>Category</Label>
							<Select.Root
								type="single"
								value={settingsGroupId}
								onValueChange={(v) => {
									if (v) handleSettingsGroupChange(v);
								}}
							>
								<Select.Trigger class="w-full">
									{settingsGroup.label}
								</Select.Trigger>
								<Select.Content>
									{#each ALERT_GROUPS as group (group.id)}
										<Select.Item value={group.id}>{group.label}</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						</div>

						<!-- Metric (only for groups with metrics) -->
						{#if settingsHasMetrics}
							<div class="grid gap-2">
								<Label>Metric</Label>
								<Select.Root
									type="single"
									value={settingsMetric || ''}
									onValueChange={(v) => {
										if (v) handleSettingsMetricChange(v);
									}}
								>
									<Select.Trigger class="w-full">
										{settingsCurrentMetric?.label ?? 'Select metric'}
									</Select.Trigger>
									<Select.Content>
										{#each settingsGroup.metrics as m (m.value)}
											<Select.Item value={m.value}>{m.label}</Select.Item>
										{/each}
									</Select.Content>
								</Select.Root>
							</div>

							<!-- Threshold -->
							{#if settingsCurrentMetric}
								<div class="grid gap-2">
									<Label for="s-threshold">
										Threshold
										<span class="ml-1 font-mono text-xs text-muted-foreground">
											({settingsCurrentMetric.unit})
										</span>
									</Label>
									<Input
										id="s-threshold"
										type="number"
										min="0"
										bind:value={settingsThreshold}
										class="font-mono"
									/>
								</div>
							{/if}
						{/if}

						<!-- Event checkboxes (only for groups with events) -->
						{#if settingsHasEvents}
							<div class="grid gap-2">
								<Label>Events</Label>
								<div class="space-y-2 rounded border border-border p-3">
									{#each settingsGroup.events as ev (ev.value)}
										<div class="flex items-center gap-2">
											<Checkbox
												id={`s-ev-${ev.value}`}
												checked={settingsEvents.has(ev.value)}
												onCheckedChange={() => toggleSettingsEvent(ev.value)}
											/>
											<label for={`s-ev-${ev.value}`} class="cursor-pointer text-sm">
												{ev.label}
											</label>
										</div>
									{/each}
								</div>
								<p class="text-xs text-muted-foreground">
									Alert fires when any of the selected events occur.
								</p>
							</div>
						{/if}

						<!-- Webhook URL -->
						<div class="grid gap-2">
							<Label for="s-webhook">
								Webhook URL
								<span class="text-muted-foreground">(optional)</span>
							</Label>
							<Input
								id="s-webhook"
								type="url"
								bind:value={settingsWebhookUrl}
								placeholder="https://hooks.example.com/alert"
								class="font-mono text-sm"
							/>
						</div>

						<!-- Notification channels -->
						<div class="grid gap-2">
							<p class="text-sm font-medium">Notification channels</p>
							<IntegrationSelector {ruleId} />
						</div>
					</div>

					<Button onclick={handleSaveSettings} disabled={settingsSaving} aria-busy={settingsSaving}>
						{settingsSaving ? 'Saving…' : 'Save settings'}
					</Button>

					<!-- Danger Zone -->
					<div class="space-y-3 rounded border border-destructive/30 p-4">
						<h2 class="text-sm font-semibold text-destructive">Danger zone</h2>
						<p class="text-sm text-muted-foreground">
							Permanently delete this alert rule. This cannot be undone.
						</p>
						<Button
							variant="outline"
							size="sm"
							class="border-destructive/50 text-destructive hover:border-destructive hover:text-destructive"
							onclick={() => (showDeleteDialog = true)}
						>
							Delete rule…
						</Button>
					</div>
				</div>
			{/if}
		</div>
	</div>

	<ConfirmDialog
		bind:open={showDeleteDialog}
		title={`Delete ${rule.name}?`}
		description="You won't be notified about these conditions any more. Your notification channels are kept."
		confirmLabel="Delete rule"
		pendingLabel="Deleting…"
		onconfirm={handleDelete}
	/>
{/if}
