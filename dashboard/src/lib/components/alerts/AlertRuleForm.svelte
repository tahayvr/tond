<!--
  AlertRuleForm — create or edit an alert rule: what to watch, where, the
  threshold, how long it must last, severity, reminders and channels.

  <AlertRuleForm {kinds} {apps} {channels} onsubmit={save} submitLabel="Create rule" />
  <AlertRuleForm {kinds} {apps} {channels} rule={rule} onsubmit={save} … />   (kind fixed)
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import type {
		AlertKind,
		AlertKindInfo,
		AlertRule,
		AlertRuleInput,
		AlertSeverity,
		App,
		Integration,
		QuietHours
	} from '$lib/api';
	import {
		ALERT_KIND_META,
		CHANNEL_KIND_LABEL,
		FOR_OPTIONS,
		KIND_GROUPS,
		REPEAT_OPTIONS,
		WINDOW_OPTIONS,
		optionLabel,
		quietHoursSummary
	} from '$lib/alerts';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { cn } from '$lib/utils';

	let {
		kinds,
		apps,
		channels,
		rule,
		globalQuiet,
		submitLabel,
		pendingLabel,
		onsubmit
	}: {
		kinds: AlertKindInfo[];
		apps: App[];
		channels: Integration[];
		/** Editing an existing rule (its kind can't change) */
		rule?: AlertRule | null;
		globalQuiet?: QuietHours;
		submitLabel: string;
		pendingLabel: string;
		onsubmit: (input: AlertRuleInput) => Promise<unknown>;
	} = $props();

	// Seeded once from the rule (or the first available kind); the form then owns it.
	const initial = untrack(() => rule ?? null);
	const firstKind = untrack(() => kinds.find((k) => k.available)?.kind ?? 'app_down');
	let kind = $state<AlertKind>(initial?.kind ?? firstKind);
	let name = $state(initial?.name ?? '');
	let appId = $state(initial?.app_id ?? '');
	let threshold = $state<number | null>(initial?.threshold ?? null);
	let windowSecs = $state(initial?.window_secs ?? 0);
	let forSecs = $state(initial?.for_secs ?? 0);
	let severity = $state<AlertSeverity>(initial?.severity ?? 'warning');
	let repeatSecs = $state(initial?.repeat_secs ?? 14_400);
	let selected = $state<string[]>(initial?.integration_ids ?? []);
	let pending = $state(false);
	let error = $state('');

	let info = $derived(kinds.find((k) => k.kind === kind));
	let meta = $derived(ALERT_KIND_META[kind]);
	let hasThreshold = $derived(meta.comparison !== 'none');

	/** Apply the kind's defaults (new rules, or when the kind changes). */
	function applyDefaults(k: AlertKindInfo | undefined) {
		if (!k) return;
		threshold = k.default_threshold;
		windowSecs = k.default_window_secs ?? 0;
		forSecs = k.default_for_secs;
		severity = k.default_severity;
	}

	if (!initial) untrack(() => applyDefaults(kinds.find((k) => k.kind === kind)));

	function handleKindChange(value: string) {
		kind = value as AlertKind;
		applyDefaults(kinds.find((k) => k.kind === kind));
		if (ALERT_KIND_META[kind].scope !== 'app') appId = '';
	}

	function toggleChannel(id: string, on: boolean) {
		selected = on ? [...selected, id] : selected.filter((s) => s !== id);
	}

	const COMPARISON_TEXT = { above: 'above', at_least: 'at least', below: 'below', none: '' };
	const SEVERITIES: { value: AlertSeverity; label: string; hint: string }[] = [
		{ value: 'info', label: 'Info', hint: 'Good to know' },
		{ value: 'warning', label: 'Warning', hint: 'Look at it soon' },
		{ value: 'critical', label: 'Critical', hint: 'Needs attention now' }
	];

	async function handleSubmit(e: SubmitEvent) {
		e.preventDefault();
		error = '';
		if (!name.trim()) {
			error = 'Give the rule a name, so you recognise it in notifications.';
			return;
		}
		if (hasThreshold && (threshold === null || Number.isNaN(Number(threshold)))) {
			error = 'Enter a threshold.';
			return;
		}
		if (selected.length === 0) {
			error = 'Choose at least one channel, so this rule notifies someone.';
			return;
		}
		pending = true;
		try {
			await onsubmit({
				name: name.trim(),
				kind,
				app_id: meta.scope === 'app' ? appId || null : null,
				...(hasThreshold ? { threshold: Number(threshold) } : {}),
				...(meta.windowed ? { window_secs: windowSecs } : {}),
				for_secs: forSecs,
				severity,
				repeat_secs: repeatSecs,
				integration_ids: selected
			});
		} catch (e: any) {
			error = e?.message ?? 'The rule could not be saved. Try again.';
		} finally {
			pending = false;
		}
	}
</script>

<form class="grid max-w-2xl gap-6" onsubmit={handleSubmit} novalidate>
	<InlineError message={error} ondismiss={() => (error = '')} />

	<div class="grid gap-2">
		<Label for="rule-kind">What to watch</Label>
		{#if initial}
			<p id="rule-kind" class="text-sm font-medium">{info?.label ?? meta.label}</p>
		{:else}
			<Select.Root type="single" value={kind} onValueChange={(v) => v && handleKindChange(v)}>
				<Select.Trigger id="rule-kind" class="w-full">{info?.label ?? meta.label}</Select.Trigger>
				<Select.Content>
					{#each KIND_GROUPS as group (group.label)}
						<Select.Group>
							<Select.GroupHeading>{group.label}</Select.GroupHeading>
							{#each kinds.filter((k) => group.scopes.includes(k.scope)) as k (k.kind)}
								<Select.Item value={k.kind} disabled={!k.available} label={k.label}>
									<span class="flex flex-col">
										<span>{k.label}</span>
										{#if !k.available}
											<span class="text-xs text-muted-foreground">Not available yet</span>
										{/if}
									</span>
								</Select.Item>
							{/each}
						</Select.Group>
					{/each}
				</Select.Content>
			</Select.Root>
		{/if}
		{#if info}
			<p class="text-xs text-muted-foreground">{info.description}</p>
			{#if !info.available && info.unavailable_reason}
				<p class="text-xs text-warn">{info.unavailable_reason}</p>
			{/if}
		{/if}
	</div>

	<div class="grid gap-2">
		<Label for="rule-name">Name</Label>
		<Input
			id="rule-name"
			bind:value={name}
			placeholder={`${meta.label} on production`}
			maxlength={100}
		/>
	</div>

	{#if meta.scope === 'app'}
		<div class="grid gap-2">
			<Label for="rule-app">Apps</Label>
			<Select.Root
				type="single"
				value={appId || 'all'}
				onValueChange={(v) => (appId = v === 'all' ? '' : (v ?? ''))}
			>
				<Select.Trigger id="rule-app" class="w-full">
					{appId ? (apps.find((a) => a.id === appId)?.name ?? 'Unknown app') : 'Every app'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="all">Every app</Select.Item>
					{#each apps as app (app.id)}
						<Select.Item value={app.id}>{app.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">
				With every app, each app gets its own alert — including apps you add later.
			</p>
		</div>
	{/if}

	{#if hasThreshold || meta.windowed}
		<div class="grid gap-4 sm:grid-cols-2">
			{#if hasThreshold}
				<div class="grid gap-2">
					<Label for="rule-threshold">
						Alert when {COMPARISON_TEXT[meta.comparison]}
					</Label>
					<div class="flex items-center gap-2">
						<Input
							id="rule-threshold"
							type="number"
							inputmode="decimal"
							step="any"
							min="0"
							bind:value={threshold}
							class="font-mono"
						/>
						<span class="shrink-0 text-sm text-muted-foreground">{meta.unit}</span>
					</div>
					{#if kind === 'cpu'}
						<p class="text-xs text-muted-foreground">100% is one full CPU core.</p>
					{/if}
				</div>
			{/if}
			{#if meta.windowed}
				<div class="grid gap-2">
					<Label for="rule-window">Within the last</Label>
					<Select.Root
						type="single"
						value={String(windowSecs)}
						onValueChange={(v) => v && (windowSecs = Number(v))}
					>
						<Select.Trigger id="rule-window" class="w-full">
							{optionLabel(WINDOW_OPTIONS, windowSecs)}
						</Select.Trigger>
						<Select.Content>
							{#each WINDOW_OPTIONS as o (o.value)}
								<Select.Item value={String(o.value)}>{o.label}</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				</div>
			{/if}
		</div>
	{/if}

	<div class="grid gap-4 sm:grid-cols-2">
		<div class="grid gap-2">
			<Label for="rule-for">Fire after it lasts</Label>
			<Select.Root
				type="single"
				value={String(forSecs)}
				onValueChange={(v) => v && (forSecs = Number(v))}
			>
				<Select.Trigger id="rule-for" class="w-full"
					>{optionLabel(FOR_OPTIONS, forSecs)}</Select.Trigger
				>
				<Select.Content>
					{#each FOR_OPTIONS as o (o.value)}
						<Select.Item value={String(o.value)}>{o.label}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">Short spikes that end sooner never notify anyone.</p>
		</div>
		<div class="grid gap-2">
			<Label for="rule-repeat">Remind while it's firing</Label>
			<Select.Root
				type="single"
				value={String(repeatSecs)}
				onValueChange={(v) => v && (repeatSecs = Number(v))}
			>
				<Select.Trigger id="rule-repeat" class="w-full">
					{optionLabel(REPEAT_OPTIONS, repeatSecs)}
				</Select.Trigger>
				<Select.Content>
					{#each REPEAT_OPTIONS as o (o.value)}
						<Select.Item value={String(o.value)}>{o.label}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">Reminders stop once someone acknowledges it.</p>
		</div>
	</div>

	<fieldset class="grid gap-2">
		<legend class="mb-2 text-sm font-medium">Severity</legend>
		<div class="grid grid-cols-3 gap-2">
			{#each SEVERITIES as s (s.value)}
				<label
					class={cn(
						'flex cursor-pointer flex-col gap-0.5 rounded-sm border px-3 py-2 text-sm transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring',
						severity === s.value ? 'border-foreground bg-accent' : 'hover:border-foreground/40'
					)}
				>
					<input
						type="radio"
						name="severity"
						value={s.value}
						class="sr-only"
						checked={severity === s.value}
						onchange={() => (severity = s.value)}
					/>
					<span class="font-medium">{s.label}</span>
					<span class="text-xs text-muted-foreground">{s.hint}</span>
				</label>
			{/each}
		</div>
		<p class="text-xs text-muted-foreground">
			{#if globalQuiet?.enabled}
				Quiet hours ({globalQuiet.start}–{globalQuiet.end}) hold back info and warning alerts{globalQuiet.critical_bypass
					? '; critical ones come through'
					: ''}.
			{:else}
				Critical alerts can come through quiet hours.
			{/if}
			<a href="/alerts?tab=settings" class="underline underline-offset-2">Quiet hours</a>
		</p>
	</fieldset>

	<fieldset class="grid gap-2" aria-describedby="rule-channels-help">
		<legend class="mb-1 text-sm font-medium">Notify</legend>
		<p id="rule-channels-help" class="text-xs text-muted-foreground">
			Pick at least one channel. You can send a test once the rule is saved.
		</p>
		{#if channels.length === 0}
			<div class="rounded-sm border border-dashed p-3 text-sm">
				<p>No notification channels yet.</p>
				<p class="mt-1 text-xs text-muted-foreground">
					<a href="/integrations" class="underline underline-offset-2">Add one</a> — ntfy, Pushover, Gotify,
					Slack, Discord, Telegram, email or a webhook.
				</p>
			</div>
		{:else}
			<ul class="divide-y rounded-sm border">
				{#each channels as channel (channel.id)}
					<li class="flex items-start gap-3 px-3 py-2">
						<Checkbox
							id={`rule-ch-${channel.id}`}
							checked={selected.includes(channel.id)}
							disabled={!channel.enabled && !selected.includes(channel.id)}
							onCheckedChange={(c) => toggleChannel(channel.id, c === true)}
						/>
						<label for={`rule-ch-${channel.id}`} class="grid min-w-0 flex-1 gap-0.5 text-sm">
							<span class="flex flex-wrap items-center justify-between gap-x-2">
								<span class="font-medium break-words">{channel.name}</span>
								<span class="text-xs text-muted-foreground">
									{CHANNEL_KIND_LABEL[channel.kind] ?? channel.kind}{channel.enabled
										? ''
										: ' · switched off'}
								</span>
							</span>
							<span class="text-xs text-muted-foreground">
								{quietHoursSummary(channel.quiet_hours ?? { mode: 'global' }, globalQuiet)}
							</span>
							{#if channel.last_error}
								<span class="text-xs break-words text-destructive">
									Last delivery failed: {channel.last_error}
								</span>
							{/if}
						</label>
					</li>
				{/each}
			</ul>
		{/if}
	</fieldset>

	<div>
		<Button type="submit" disabled={pending || selected.length === 0} aria-busy={pending}>
			{pending ? pendingLabel : submitLabel}
		</Button>
		{#if selected.length === 0}
			<p class="mt-2 text-xs text-muted-foreground">Choose a channel to continue.</p>
		{/if}
	</div>
</form>
