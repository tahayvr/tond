<!--
  Cron schedule + time zone inputs with a plain-English description and the
  next five runs computed by the server (so what you see is what runs).
-->
<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { previewCronSchedule } from '$lib/api';
	import { describeCron, timeZones } from '$lib/cron-schedule';
	import { errorMessage } from '$lib/action.svelte';

	let {
		schedule = $bindable(''),
		timezone = $bindable(''),
		idPrefix = 'schedule',
		disabled = false
	}: {
		schedule?: string;
		/** IANA zone; empty string means UTC. */
		timezone?: string;
		idPrefix?: string;
		disabled?: boolean;
	} = $props();

	const PRESETS = [
		{ label: 'Every 5 minutes', value: '*/5 * * * *' },
		{ label: 'Every hour', value: '0 * * * *' },
		{ label: 'Every 6 hours', value: '0 */6 * * *' },
		{ label: 'Every day at 02:00', value: '0 2 * * *' },
		{ label: 'Weekdays at 09:00', value: '0 9 * * 1-5' },
		{ label: 'Sundays at midnight', value: '0 0 * * 0' },
		{ label: 'First of the month', value: '0 0 1 * *' }
	];

	const zones = timeZones();

	let description = $derived(describeCron(schedule));
	let nextRuns = $state<string[]>([]);
	let previewError = $state('');
	let previewLoading = $state(false);

	// Ask the server for the next runs, debounced while typing.
	$effect(() => {
		const expr = schedule.trim();
		const tz = timezone;
		nextRuns = [];
		previewError = '';
		if (!expr || !describeCron(expr).ok) return;
		let cancelled = false;
		const timer = setTimeout(async () => {
			previewLoading = true;
			try {
				const preview = await previewCronSchedule(expr, tz || null);
				if (!cancelled) nextRuns = preview.next_runs;
			} catch (e) {
				if (!cancelled) previewError = errorMessage(e);
			} finally {
				if (!cancelled) previewLoading = false;
			}
		}, 350);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	});

	function formatRun(iso: string): string {
		const date = new Date(iso);
		const opts: Intl.DateTimeFormatOptions = {
			weekday: 'short',
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit',
			hourCycle: 'h23'
		};
		try {
			return date.toLocaleString(undefined, { ...opts, timeZone: timezone || 'UTC' });
		} catch {
			return date.toLocaleString(undefined, opts);
		}
	}
</script>

<div class="grid gap-2">
	<Label for={`${idPrefix}-expr`}>Schedule</Label>
	<div class="flex flex-col gap-2 sm:flex-row">
		<Input
			id={`${idPrefix}-expr`}
			type="text"
			bind:value={schedule}
			placeholder="0 2 * * *"
			class="flex-1 font-mono"
			autocomplete="off"
			spellcheck={false}
			aria-describedby={`${idPrefix}-desc`}
			aria-invalid={schedule.trim() !== '' && !description.ok}
			{disabled}
		/>
		<Select.Root
			type="single"
			value=""
			onValueChange={(v) => {
				if (v) schedule = v;
			}}
		>
			<Select.Trigger class="w-full text-xs sm:w-44" aria-label="Schedule presets" {disabled}>
				Presets
			</Select.Trigger>
			<Select.Content>
				{#each PRESETS as preset (preset.value)}
					<Select.Item value={preset.value} class="text-xs">
						<span>{preset.label}</span>
						<span class="ml-2 font-mono text-muted-foreground">{preset.value}</span>
					</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>
	<p
		id={`${idPrefix}-desc`}
		class={description.ok || !schedule.trim() ? 'text-sm text-foreground' : 'text-sm text-destructive'}
		aria-live="polite"
	>
		{schedule.trim() ? description.text : 'minute hour day-of-month month day-of-week'}
	</p>
</div>

<div class="grid gap-2">
	<Label for={`${idPrefix}-tz`}>Time zone</Label>
	<select
		id={`${idPrefix}-tz`}
		bind:value={timezone}
		{disabled}
		class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm dark:bg-input/30"
	>
		<option value="">UTC (default)</option>
		{#each zones as zone (zone)}
			{#if zone !== 'UTC'}
				<option value={zone}>{zone.replaceAll('_', ' ')}</option>
			{/if}
		{/each}
	</select>
	<p class="text-xs text-muted-foreground">The schedule is evaluated in this time zone.</p>
</div>

{#if description.ok}
	<div class="rounded-sm border px-3 py-2 text-sm" aria-live="polite">
		<p class="text-xs font-medium text-muted-foreground">
			Next runs ({timezone || 'UTC'})
		</p>
		{#if previewError}
			<p class="mt-1 text-destructive">{previewError}</p>
		{:else if nextRuns.length}
			<ol class="mt-1 grid gap-0.5 font-mono text-xs">
				{#each nextRuns as run (run)}
					<li>{formatRun(run)}</li>
				{/each}
			</ol>
		{:else if previewLoading}
			<p class="mt-1 text-xs text-muted-foreground">Calculating…</p>
		{/if}
	</div>
{/if}
