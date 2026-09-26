<!--
  RetentionSettingsForm — how long Tond keeps build logs, cron runs, app
  events and the activity log, next to what the database weighs and where
  the space goes. Loads its own data (GET /api/settings/retention).
-->
<script lang="ts">
	import {
		getRetention,
		updateRetention,
		type RetentionField,
		type RetentionSettings
	} from '$lib/api';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { formatBytes } from '$lib/metrics-format';
	import {
		RETENTION_GROUPS,
		retentionChanges,
		retentionCopy,
		sizeBreakdown,
		validateRetention
	} from '$lib/retention';
	import { formatRelativePast } from '$lib/time';
	import { cn } from '$lib/utils';
	import ArchiveIcon from '~icons/ri/archive-stack-line';
	import DatabaseIcon from '~icons/ri/database-2-line';

	let { canEdit }: { canEdit: boolean } = $props();

	let data = $state<RetentionSettings | null>(null);
	let values = $state<Record<string, number | null>>({});
	let loadError = $state('');
	let saving = $state(false);
	let error = $state('');
	let saved = $state('');

	let fields = $derived(data?.fields ?? []);
	let problems = $derived(
		Object.fromEntries(fields.map((f) => [f.key, validateRetention(f, values[f.key] ?? null)]))
	);
	let changes = $derived(retentionChanges(fields, values));
	let dirty = $derived(Object.keys(changes).length > 0);
	let valid = $derived(Object.values(problems).every((p) => p === null));
	let db = $derived(data?.database ?? null);
	let rows = $derived(db ? sizeBreakdown(db.tables) : []);

	const BAR_COLORS = [
		'bg-chart-1',
		'bg-chart-2',
		'bg-chart-3',
		'bg-chart-4',
		'bg-chart-5',
		'bg-foreground/40',
		'bg-muted-foreground/30'
	];

	function apply(next: RetentionSettings) {
		data = next;
		values = Object.fromEntries(next.fields.map((f) => [f.key, f.value]));
	}

	async function load() {
		try {
			apply(await getRetention());
			loadError = '';
		} catch (e: any) {
			loadError = e.message;
		}
	}

	$effect(() => {
		load();
	});

	function fieldsIn(group: string): RetentionField[] {
		return fields.filter((f) => retentionCopy(f.key).group === group);
	}

	function handleReset() {
		values = Object.fromEntries(fields.map((f) => [f.key, f.default]));
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';
		saved = '';
		if (!valid) {
			error = 'Fix the highlighted values first.';
			return;
		}
		saving = true;
		try {
			apply(await updateRetention(changes));
			saved = 'Saved. The cleanup runs hourly, so older data goes within the hour.';
		} catch (err: any) {
			error = err.message;
		} finally {
			saving = false;
		}
	}
</script>

<form
	class="rounded-lg border border-border bg-card p-4"
	onsubmit={handleSubmit}
	novalidate
	aria-labelledby="retention-title"
>
	<div class="mb-1 flex items-center gap-2">
		<ArchiveIcon class="size-4 text-muted-foreground" aria-hidden="true" />
		<h2 id="retention-title" class="font-semibold">Data retention</h2>
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		How long Tond keeps history. Keeping less makes the database smaller and backups faster. Metrics
		keep fixed tiers (1 minute for 48 hours, hourly for 90 days).
	</p>

	<InlineError message={loadError} />

	{#if !data && !loadError}
		<div class="grid gap-3 lg:grid-cols-2">
			<Skeleton class="h-48" />
			<Skeleton class="h-48" />
		</div>
	{:else if data}
		<div class="grid gap-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
			<div class="space-y-5">
				{#each RETENTION_GROUPS as group (group.id)}
					{@const groupFields = fieldsIn(group.id)}
					{#if groupFields.length > 0}
						<fieldset class="space-y-3">
							<legend
								class="mb-2 text-xs font-medium tracking-wide text-muted-foreground uppercase"
							>
								{group.title}
							</legend>
							{#each groupFields as field (field.key)}
								{@const copy = retentionCopy(field.key)}
								{@const problem = problems[field.key]}
								<div class="grid gap-1">
									<div class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
										<label for={field.key}>{copy.label}</label>
										<span class="inline-flex items-center gap-2">
											<Input
												id={field.key}
												type="number"
												inputmode="numeric"
												min={field.min}
												max={field.max}
												class={cn('h-8 w-24', problem && 'border-destructive')}
												bind:value={values[field.key]}
												disabled={!canEdit || saving}
												aria-invalid={problem ? 'true' : undefined}
												aria-describedby={`${field.key}-help`}
											/>
											<span class="text-muted-foreground">{field.unit}</span>
										</span>
									</div>
									<p
										id={`${field.key}-help`}
										class={cn('text-xs', problem ? 'text-destructive' : 'text-muted-foreground')}
									>
										{problem ?? copy.help}
										{#if !problem && field.value !== field.default}
											Default {field.default}.
										{/if}
									</p>
								</div>
							{/each}
						</fieldset>
					{/if}
				{/each}
			</div>

			<section aria-labelledby="db-size-title" class="space-y-3">
				<div class="flex items-center gap-2">
					<DatabaseIcon class="size-4 text-muted-foreground" aria-hidden="true" />
					<h3 id="db-size-title" class="text-sm font-medium">Database size</h3>
				</div>
				{#if db}
					<p class="text-2xl font-semibold tabular-nums">
						{formatBytes(db.file_bytes + db.wal_bytes)}
					</p>
					<p class="text-xs text-muted-foreground">
						tond.db {formatBytes(db.file_bytes)}{#if db.wal_bytes > 0}{' '}+ write-ahead log {formatBytes(
								db.wal_bytes
							)}{/if}{#if db.free_bytes > 0}, {formatBytes(db.free_bytes)} free inside the file{/if}.
						Measured {formatRelativePast(db.measured_at)}.
					</p>
					{#if rows.length > 0}
						<div class="flex h-2.5 overflow-hidden rounded-full bg-muted" aria-hidden="true">
							{#each rows as row, i (row.label)}
								<div
									class={BAR_COLORS[i % BAR_COLORS.length]}
									style={`width: ${row.percent}%`}
								></div>
							{/each}
						</div>
						<ul class="space-y-1.5 text-sm">
							{#each rows as row, i (row.label)}
								<li class="flex items-center gap-2">
									<span
										class={cn('size-2.5 shrink-0 rounded-sm', BAR_COLORS[i % BAR_COLORS.length])}
										aria-hidden="true"
									></span>
									<span class="min-w-0 flex-1 truncate">{row.label}</span>
									<span class="text-muted-foreground tabular-nums">{formatBytes(row.bytes)}</span>
									<span class="w-10 text-right text-xs text-muted-foreground tabular-nums"
										>{Math.round(row.percent)}%</span
									>
								</li>
							{/each}
						</ul>
					{:else}
						<p class="text-xs text-muted-foreground">A per-table breakdown isn't available.</p>
					{/if}
				{:else}
					<p class="text-sm text-muted-foreground">Couldn't measure the database right now.</p>
				{/if}
			</section>
		</div>

		<InlineError message={error} ondismiss={() => (error = '')} class="mt-4" />
		<SuccessAlert message={saved} timeout={5000} ondismiss={() => (saved = '')} class="mt-4" />

		<div class="mt-4 flex flex-wrap items-center gap-3">
			{#if canEdit}
				<Button type="submit" size="sm" disabled={saving || !dirty || !valid}>
					{saving ? 'Saving…' : 'Save retention'}
				</Button>
				<Button type="button" variant="ghost" size="sm" onclick={handleReset} disabled={saving}>
					Use defaults
				</Button>
			{:else}
				<p class="text-xs text-muted-foreground">Only admins can change retention.</p>
			{/if}
		</div>
	{/if}
</form>
