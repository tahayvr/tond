<script lang="ts">
	import { page } from '$app/state';
	import {
		updateAlertRule,
		deleteAlertRule,
		ALERT_GROUPS,
		type AlertRule,
		type AlertMetric,
		type AlertEvent,
		type App
	} from '$lib/api';
	import { SvelteSet } from 'svelte/reactivity';
	import { goto } from '$app/navigation';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
	import IntegrationSelector from '$lib/components/integrations/integration-selector.svelte';
	import Bell from '@lucide/svelte/icons/bell';
	import Plug from '@lucide/svelte/icons/plug';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();

	let rule = $derived(data.rule as AlertRule | null);
	let error = $state('');
	let showDeleteDialog = $state(false);

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
	let settingsSuccess = $state(false);

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

	async function handleToggleEnabled() {
		if (!rule) return;
		try {
			rule = await updateAlertRule(ruleId, { enabled: !rule.enabled });
		} catch (e: any) {
			error = e.message ?? 'Failed to update rule';
		}
	}

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
		settingsSuccess = false;
		try {
			rule = await updateAlertRule(ruleId, {
				name: settingsName.trim(),
				metric: settingsHasMetrics && settingsMetric ? (settingsMetric as AlertMetric) : null,
				threshold: settingsThreshold,
				webhook_url: settingsWebhookUrl.trim(),
				events: [...settingsEvents]
			});
			settingsSuccess = true;
			setTimeout(() => (settingsSuccess = false), 3000);
		} catch (e: any) {
			settingsError = e.message ?? 'Failed to save settings';
		} finally {
			settingsSaving = false;
		}
	}

	async function handleDelete() {
		await deleteAlertRule(ruleId);
		goto('/alerts');
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
	<div class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-destructive">
		{error}
	</div>
{:else if rule}
	<div class="space-y-6">
		<!-- Breadcrumb -->
		<Breadcrumb items={[{ label: 'Alerts', href: '/alerts' }, { label: rule.name }]} />

		<!-- Error banner -->
		{#if error}
			<div
				class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-sm text-destructive"
			>
				{error}
			</div>
		{/if}

		<!-- Header -->
		<div class="flex h-14 items-center justify-between">
			<div>
				<h2 class="text-3xl font-extrabold uppercase">{rule.name}</h2>
				<div class="mt-2 flex items-center gap-3 text-sm text-muted-foreground">
					<Badge variant={rule.enabled ? 'default' : 'secondary'}>
						{rule.enabled ? 'Enabled' : 'Paused'}
					</Badge>
					{#if rule.metric}
						<span class="font-mono text-xs uppercase">{metricLabel(rule.metric)}</span>
						<span class="font-mono text-xs">
							&gt; {rule.threshold}{thresholdUnit(rule.metric)}
						</span>
					{/if}
					{#if rule.events.length > 0}
						<span class="font-mono text-xs uppercase">
							{rule.events.length} event{rule.events.length > 1 ? 's' : ''}
						</span>
					{/if}
				</div>
			</div>
			<div class="flex gap-2">
				<Button
					variant="outline"
					size="sm"
					onclick={handleToggleEnabled}
					class={rule.enabled
						? 'border-destructive/50 text-destructive hover:border-destructive hover:text-destructive'
						: ''}
				>
					{rule.enabled ? 'Disable' : 'Enable'}
				</Button>
				<Button variant="destructive" size="sm" onclick={() => (showDeleteDialog = true)}>
					Delete
				</Button>
			</div>
		</div>

		<Separator />

		<!-- Tabs -->
		<Tabs.Root value="overview">
			<Tabs.List>
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="settings">Settings</Tabs.Trigger>
			</Tabs.List>

			<!-- ── Overview tab ── -->
			<Tabs.Content value="overview">
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
									<Badge variant={rule.enabled ? 'default' : 'secondary'} class="text-xs">
										{rule.enabled ? 'Enabled' : 'Paused'}
									</Badge>
								</div>
								<div>
									<span class="text-muted-foreground">Last fired:</span>
									{formatRelative(rule.last_fired_at)}
									{#if rule.last_fired_at}
										<span class="ml-1 font-mono text-xs text-muted-foreground/60">
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
			</Tabs.Content>

			<!-- ── Settings tab ── -->
			<Tabs.Content value="settings">
				<div class="space-y-6 pt-4">
					{#if settingsError}
						<div
							class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-sm text-destructive"
						>
							{settingsError}
						</div>
					{/if}
					{#if settingsSuccess}
						<div
							class="rounded-md border border-green-500/50 bg-green-500/10 p-4 text-sm text-green-600 dark:text-green-400"
						>
							Settings saved.
						</div>
					{/if}

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
							<Label>Notification Channels</Label>
							<IntegrationSelector {ruleId} />
						</div>
					</div>

					<Button
						onclick={handleSaveSettings}
						disabled={settingsSaving}
						variant="outline"
						class="border-success text-success"
					>
						{settingsSaving ? 'Saving...' : 'Save Settings'}
					</Button>

					<!-- Danger Zone -->
					<div class="space-y-3 rounded border border-destructive/30 p-4">
						<h4 class="font-mono text-xs font-medium tracking-wider text-destructive uppercase">
							Danger Zone
						</h4>
						<p class="text-sm text-muted-foreground">
							Permanently delete this alert rule. This cannot be undone.
						</p>
						<Button
							variant="outline"
							size="sm"
							class="border-destructive/50 text-destructive hover:border-destructive hover:text-destructive"
							onclick={() => (showDeleteDialog = true)}
						>
							Delete Rule
						</Button>
					</div>
				</div>
			</Tabs.Content>
		</Tabs.Root>
	</div>

	<ConfirmDeleteDialog
		bind:open={showDeleteDialog}
		title="Delete Alert Rule"
		description="This will permanently delete the alert rule and cannot be undone."
		confirmation={rule.name}
		onconfirm={handleDelete}
	/>
{/if}
