<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		createAlertRule,
		ALERT_GROUPS,
		type App,
		type AlertMetric,
		type AlertEvent
	} from '$lib/api';
	import { SvelteSet } from 'svelte/reactivity';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import Bell from '@lucide/svelte/icons/bell';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let loading = $state(false);
	let error = $state('');
	let apps = $derived(data.apps as App[]);

	let name = $state('');
	let selectedGroupId = $state(ALERT_GROUPS[0].id);
	// For metric groups: which metric is selected
	let selectedMetric = $state<AlertMetric | ''>('cpu');
	// Per-metric threshold
	let threshold = $state(80);
	// For event groups: which events are checked
	let selectedEvents = new SvelteSet<AlertEvent>();
	let selectedAppId = $state('');
	let webhookUrl = $state('');

	// Derived: the current group object
	let currentGroup = $derived(
		ALERT_GROUPS.find((g) => g.id === selectedGroupId) ?? ALERT_GROUPS[0]
	);

	// Whether this group has metrics at all
	let hasMetrics = $derived(currentGroup.metrics.length > 0);
	// Whether this group has events at all
	let hasEvents = $derived(currentGroup.events.length > 0);

	// The currently selected metric object
	let currentMetric = $derived(
		currentGroup.metrics.find((m) => m.value === selectedMetric) ?? currentGroup.metrics[0]
	);

	// App Down requires a specific app
	let requiresApp = $derived(selectedMetric === 'app_down');

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});

	function handleGroupChange(groupId: string) {
		selectedGroupId = groupId;
		const group = ALERT_GROUPS.find((g) => g.id === groupId) ?? ALERT_GROUPS[0];
		// Reset metric to first in new group
		selectedMetric = group.metrics[0]?.value ?? '';
		// Reset threshold to first metric's placeholder
		threshold = group.metrics[0]?.placeholder ?? 80;
		// Clear event selections
		selectedEvents.clear();
	}

	function handleMetricChange(metric: string) {
		selectedMetric = metric as AlertMetric;
		const m = currentGroup.metrics.find((x) => x.value === metric);
		if (m) threshold = m.placeholder;
	}

	function toggleEvent(event: AlertEvent) {
		if (selectedEvents.has(event)) {
			selectedEvents.delete(event);
		} else {
			selectedEvents.add(event);
		}
	}

	async function handleCreate() {
		if (!name.trim()) {
			error = 'Name is required';
			return;
		}
		if (hasMetrics && !selectedMetric) {
			error = 'Please select a metric';
			return;
		}
		if (selectedMetric === 'app_down' && !selectedAppId) {
			error = 'App Down alerts require a specific app to be selected';
			return;
		}
		if (hasEvents && selectedEvents.size === 0 && !hasMetrics) {
			error = 'Please select at least one event';
			return;
		}
		error = '';
		loading = true;
		try {
			const created = await createAlertRule({
				name: name.trim(),
				...(hasMetrics && selectedMetric
					? { metric: selectedMetric as AlertMetric, threshold }
					: {}),
				...(selectedAppId ? { app_id: selectedAppId } : {}),
				...(selectedEvents.size > 0 ? { events: [...selectedEvents] } : {}),
				webhook_url: webhookUrl.trim()
			});
			goto(`/alerts/${created.id}`);
		} catch (e: any) {
			error = e.message ?? 'Failed to create alert rule';
			loading = false;
		}
	}
</script>

<div class="space-y-6 pb-12">
	<Breadcrumb items={[{ label: 'Alerts', href: '/alerts' }, { label: 'New Alert Rule' }]} />

	{#if error}
		<div
			class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-sm text-destructive"
		>
			{error}
		</div>
	{/if}

	<div class="space-y-1">
		<h2 class="text-3xl font-extrabold uppercase">New Alert Rule</h2>
		<p class="text-sm text-muted-foreground">
			Get notified when a metric crosses a threshold or an event occurs.
		</p>
	</div>

	<div class="grid max-w-xl gap-5">
		<!-- Name -->
		<div class="grid gap-2">
			<Label for="name">Name</Label>
			<Input id="name" type="text" bind:value={name} placeholder="High CPU on api-service" />
		</div>

		<!-- Group / Category -->
		<div class="grid gap-2">
			<Label>Category</Label>
			<Select.Root
				type="single"
				value={selectedGroupId}
				onValueChange={(v) => {
					if (v) handleGroupChange(v);
				}}
			>
				<Select.Trigger class="w-full">
					{currentGroup.label}
				</Select.Trigger>
				<Select.Content>
					{#each ALERT_GROUPS as group (group.id)}
						<Select.Item value={group.id}>{group.label}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>

		<!-- Metric picker (only for groups with metrics) -->
		{#if hasMetrics}
			<div class="grid gap-2">
				<Label>Metric</Label>
				<Select.Root
					type="single"
					value={selectedMetric || ''}
					onValueChange={(v) => {
						if (v) handleMetricChange(v);
					}}
				>
					<Select.Trigger class="w-full">
						{currentMetric?.label ?? 'Select metric'}
					</Select.Trigger>
					<Select.Content>
						{#each currentGroup.metrics as m (m.value)}
							<Select.Item value={m.value}>{m.label}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>

			<!-- Threshold -->
			{#if currentMetric}
				<div class="grid gap-2">
					<Label for="threshold">
						Threshold
						<span class="ml-1 font-mono text-xs text-muted-foreground">({currentMetric.unit})</span>
					</Label>
					<Input
						id="threshold"
						type="number"
						min="0"
						bind:value={threshold}
						placeholder={String(currentMetric.placeholder)}
						class="font-mono"
					/>
					<p class="text-xs text-muted-foreground">
						{#if selectedMetric === 'app_down'}
							Alert fires when the app has been non-running for this many minutes.
						{:else}
							Alert fires when {currentMetric.label.toLowerCase()} exceeds this percentage.
						{/if}
					</p>
				</div>
			{/if}

			<!-- App scope (for metric groups) -->
			<div class="grid gap-2">
				<Label>
					App
					{#if !requiresApp}
						<span class="text-muted-foreground">(optional)</span>
					{/if}
				</Label>
				<Select.Root
					type="single"
					value={selectedAppId || '__global__'}
					onValueChange={(v) => {
						selectedAppId = v === '__global__' ? '' : (v ?? '');
					}}
				>
					<Select.Trigger class="w-full">
						{#if selectedAppId}
							{apps.find((a) => a.id === selectedAppId)?.name ?? selectedAppId.slice(0, 8)}
						{:else}
							Global — all apps
						{/if}
					</Select.Trigger>
					<Select.Content>
						{#if !requiresApp}
							<Select.Item value="__global__">Global — all apps</Select.Item>
						{/if}
						{#each apps as app (app.id)}
							<Select.Item value={app.id}>{app.name}</Select.Item>
						{/each}
						{#if apps.length === 0 && requiresApp}
							<div class="px-3 py-2 text-xs text-muted-foreground">No apps found</div>
						{/if}
					</Select.Content>
				</Select.Root>
				{#if requiresApp}
					<p class="text-xs text-muted-foreground">App Down alerts require a specific app.</p>
				{/if}
			</div>
		{/if}

		<!-- Event checkboxes (only for groups with events) -->
		{#if hasEvents}
			<div class="grid gap-2">
				<Label>Events</Label>
				<div class="space-y-2 rounded border border-border p-3">
					{#each currentGroup.events as ev (ev.value)}
						<div class="flex items-center gap-2">
							<Checkbox
								id={`ev-${ev.value}`}
								checked={selectedEvents.has(ev.value)}
								onCheckedChange={() => toggleEvent(ev.value)}
							/>
							<label for={`ev-${ev.value}`} class="cursor-pointer text-sm">{ev.label}</label>
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
			<Label for="webhook">
				Webhook URL
				<span class="text-muted-foreground">(optional)</span>
			</Label>
			<Input
				id="webhook"
				type="url"
				bind:value={webhookUrl}
				placeholder="https://hooks.example.com/alert"
				class="font-mono text-sm"
			/>
			<p class="text-xs text-muted-foreground">
				A POST request with the alert payload will be sent to this URL when the rule fires. After
				creating the rule, link Telegram, Discord, Slack, or Email channels from the rule's Settings
				tab.
			</p>
		</div>
	</div>

	<div>
		<Button
			onclick={handleCreate}
			disabled={loading}
			variant="outline"
			class="border-success text-success"
		>
			{#if loading}
				<span class="animate-pulse">Creating...</span>
			{:else}
				<Bell class="mr-2 h-4 w-4" />
				Create Alert Rule
			{/if}
		</Button>
	</div>
</div>
