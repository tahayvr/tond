<!--
  Automatic backups of one database: its own schedule (UTC) and how many
  scheduled backups to keep, the next run, the latest result, and a warning
  when nothing backs the database up automatically.
-->
<script lang="ts">
	import {
		getDatabaseBackupSchedule,
		updateDatabaseBackupSchedule,
		type DatabaseBackupStatus
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { describeCron } from '$lib/cron-schedule';
	import { BACKUP_PRESETS, describeBackupSchedule, presetFor } from '$lib/database-ops';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import AlertIcon from '~icons/ri/alert-line';
	import TimeIcon from '~icons/ri/time-line';

	interface Props {
		dbId: string;
		dbName: string;
		/** db:write */
		canEdit: boolean;
		/** Bumped by the parent after a backup, so "latest backup" refreshes. */
		refreshKey?: number;
	}

	let { dbId, dbName, canEdit, refreshKey = 0 }: Props = $props();

	let status = $state<DatabaseBackupStatus | null>(null);
	let loadError = $state('');
	let preset = $state('off');
	let custom = $state('');
	let retention = $state<number | null>(7);

	function applyStatus(s: DatabaseBackupStatus) {
		status = s;
		preset = presetFor(s.schedule);
		custom = preset === 'custom' ? (s.schedule ?? '') : '';
		retention = s.retention;
	}

	async function load() {
		loadError = '';
		try {
			applyStatus(await getDatabaseBackupSchedule(dbId));
		} catch (e: any) {
			loadError = e.message ?? 'Could not load the backup schedule';
		}
	}

	$effect(() => {
		void dbId;
		void refreshKey;
		void load();
	});

	let schedule = $derived(preset === 'off' ? null : preset === 'custom' ? custom.trim() : preset);
	let customDescription = $derived(preset === 'custom' ? describeCron(custom) : null);
	let retentionOk = $derived(
		retention !== null && Number.isInteger(retention) && retention >= 1 && retention <= 365
	);
	let dirty = $derived(
		!!status && (schedule !== status.schedule || (retention ?? 0) !== status.retention)
	);
	let valid = $derived(
		retentionOk && (preset !== 'custom' || (!!custom.trim() && !!customDescription?.ok))
	);

	const save = useAction(
		() => updateDatabaseBackupSchedule(dbId, { schedule, retention: retention ?? undefined }),
		{
			success: (s) =>
				s.schedule
					? `Backups of ${dbName} scheduled: ${describeBackupSchedule(s.schedule)}.`
					: `Scheduled backups of ${dbName} are off.`,
			onSuccess: (s) => applyStatus(s)
		}
	);

	function when(iso: string): string {
		return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
	}

	const selectLabel = $derived(
		preset === 'off'
			? 'Off'
			: preset === 'custom'
				? 'Custom schedule'
				: (BACKUP_PRESETS.find((p) => p.value === preset)?.label ?? preset)
	);
</script>

<section
	class="space-y-4 rounded-lg border p-4"
	aria-labelledby="db-backup-schedule-heading"
	data-testid="database-backup-schedule"
>
	<div class="flex flex-wrap items-start justify-between gap-2">
		<div class="space-y-1">
			<h2
				id="db-backup-schedule-heading"
				class="text-sm font-semibold tracking-wide text-muted-foreground uppercase"
			>
				Automatic backups
			</h2>
			<p class="max-w-prose text-sm text-muted-foreground">
				Back up {dbName} on its own schedule, in addition to full-server snapshots. Failed backups trigger
				the "backup failed" alert.
			</p>
		</div>
	</div>

	<InlineError message={loadError} />

	{#if status?.warning}
		<div
			class="flex items-start gap-2 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn"
			role="note"
		>
			<AlertIcon class="mt-0.5 size-4 shrink-0" />
			<span>{status.warning}</span>
		</div>
	{/if}

	{#if status}
		<div class="grid gap-4 sm:grid-cols-[minmax(0,24rem)_10rem]">
			<div class="grid gap-2">
				<Label for="db-backup-preset">Schedule</Label>
				<Select.Root type="single" bind:value={preset} disabled={!canEdit || save.pending}>
					<Select.Trigger id="db-backup-preset" class="w-full" aria-label="Backup schedule">
						{selectLabel}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="off">Off</Select.Item>
						{#each BACKUP_PRESETS as p (p.value)}
							<Select.Item value={p.value}>
								<span>{p.label}</span>
								<span class="ml-2 font-mono text-xs text-muted-foreground">{p.value}</span>
							</Select.Item>
						{/each}
						<Select.Item value="custom">Custom schedule…</Select.Item>
					</Select.Content>
				</Select.Root>
				{#if preset === 'custom'}
					<Input
						id="db-backup-cron"
						bind:value={custom}
						placeholder="30 2 * * *"
						class="font-mono"
						autocomplete="off"
						spellcheck={false}
						aria-label="Cron expression (UTC)"
						aria-invalid={!!custom.trim() && !customDescription?.ok}
						disabled={!canEdit || save.pending}
					/>
					<p
						class={custom.trim() && !customDescription?.ok
							? 'text-xs text-destructive'
							: 'text-xs text-muted-foreground'}
						aria-live="polite"
					>
						{custom.trim()
							? `${customDescription?.text}${customDescription?.ok ? ' (UTC)' : ''}`
							: 'minute hour day-of-month month day-of-week, in UTC'}
					</p>
				{/if}
			</div>
			<div class="grid content-start gap-2">
				<Label for="db-backup-retention">Keep</Label>
				<div class="flex items-center gap-2">
					<Input
						id="db-backup-retention"
						type="number"
						min={1}
						max={365}
						bind:value={retention}
						class="w-20"
						aria-invalid={!retentionOk}
						disabled={!canEdit || save.pending}
					/>
					<span class="text-sm text-muted-foreground">backups</span>
				</div>
				{#if !retentionOk}
					<p class="text-xs text-destructive">Keep between 1 and 365.</p>
				{/if}
			</div>
		</div>

		<dl class="grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
			<div class="flex flex-wrap gap-x-2">
				<dt class="text-muted-foreground">Next backup</dt>
				<dd class="font-medium">
					{status.next_run_at ? when(status.next_run_at) : '—'}
				</dd>
			</div>
			<div class="flex flex-wrap gap-x-2">
				<dt class="text-muted-foreground">Latest backup</dt>
				<dd class="font-medium">
					{status.last_backup_at ? when(status.last_backup_at) : 'None yet'}
				</dd>
			</div>
			{#if status.full_snapshots_enabled}
				<div class="flex flex-wrap gap-x-2 sm:col-span-2">
					<dt class="text-muted-foreground">Full snapshots</dt>
					<dd>{describeBackupSchedule(status.full_schedule)} (include {dbName})</dd>
				</div>
			{/if}
		</dl>

		{#if status.last_status === 'failed'}
			<InlineError
				title="The latest backup of {dbName} failed"
				message={status.last_error ?? 'See the server logs for details.'}
			/>
		{/if}

		{#if canEdit}
			<div class="flex flex-wrap items-center gap-3">
				<Button size="sm" disabled={!dirty || !valid || save.pending} onclick={() => save.run()}>
					<TimeIcon class="size-4" />
					{save.pending ? 'Saving…' : 'Save schedule'}
				</Button>
				<SuccessAlert message={save.success} ondismiss={save.clear} timeout={6000} />
			</div>
			<InlineError message={save.error} ondismiss={save.clear} />
		{/if}
	{:else if !loadError}
		<p class="text-xs text-muted-foreground">Loading the schedule…</p>
	{/if}
</section>
