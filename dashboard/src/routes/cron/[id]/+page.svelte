<script lang="ts">
	import { page } from '$app/state';
	import {
		getCronJob,
		updateCronJob,
		deleteCronJob,
		triggerCronJob,
		listCronJobRuns,
		listProjects,
		type CronJob,
		type CronJobRun,
		type Project
	} from '$lib/api';
	import { isAdmin } from '$lib/auth.svelte';
	import CronVolumeFields, {
		OWN_FOLDER,
		cronVolumeLabel,
		validateCronVolume
	} from '$lib/components/forms/cron-volume-fields.svelte';
	import type { CronVolumeInput } from '$lib/api';
	import { intOr, optionalInt } from '$lib/form-numbers';
	import { goto } from '$app/navigation';
	import {
		setUiContext,
		clearUiContext,
		cronJobContext,
		openFeedPanel
	} from '$lib/ui-context.svelte';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import Play from '~icons/ri/play-line';
	import RotateCw from '~icons/ri/restart-line';
	import ChevronDown from '~icons/ri/arrow-down-s-line';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import Plus from '~icons/ri/add-line';
	import Trash2 from '~icons/ri/delete-bin-line';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();
	type CronJobDetailData = {
		job: CronJob;
		projects: Project[];
	};

	let job = $state<CronJob | null>(null);
	let error = $state('');
	let showDeleteDialog = $state(false);
	let triggering = $state(false);
	let triggerError = $state('');

	// Runs tab
	let runs = $state<CronJobRun[]>([]);
	let runsLoading = $state(false);
	let runsError = $state('');
	let expandedRunId = $state<string | null>(null);
	let runsOffset = $state(0);
	const RUNS_PAGE_SIZE = 20;
	let runsHasMore = $state(false);
	let runsLoadingMore = $state(false);

	// Settings form
	let settingsName = $state('');
	let settingsSchedule = $state('');
	let settingsImage = $state('');
	let settingsCommand = $state('');
	// Number inputs bind as number | null (null when empty).
	let settingsTimeout = $state<number | null>(null);
	let settingsEnvVars = $state<Record<string, string>>({});
	let settingsConcurrencyPolicy = $state('skip');
	let settingsMemoryMb = $state<number | null>(null);
	let settingsCpuShares = $state<number | null>(null);
	let settingsTimezone = $state('');
	let settingsMaxRetries = $state<number | null>(0);
	let settingsRetryDelay = $state<number | null>(60);
	// volumes in settings
	let settingsVolumes = $state<CronVolumeInput[]>([]);
	// project assignment
	let projects = $state<Project[]>([]);
	let settingsProjectId = $state<string | null>(null);

	let settingsSaving = $state(false);
	let settingsError = $state('');
	let settingsSuccess = $state('');

	// Env var dialog (settings tab)
	let envDialogOpen = $state(false);
	let envEditingKey = $state<string | null>(null);
	let envNewKey = $state('');
	let envNewValue = $state('');
	let envDialogError = $state('');

	// Volume dialog (settings tab)
	let volDialogOpen = $state(false);
	let volEditingIdx = $state<number | null>(null);
	let volSource = $state(OWN_FOLDER);
	let volName = $state('');
	let volContainerPath = $state('');
	let volDialogError = $state('');

	let envEntries = $derived(Object.entries(settingsEnvVars));

	let jobId = $derived(page.params.id || params.id);

	async function refreshJobData() {
		error = '';
		try {
			[job, projects] = await Promise.all([getCronJob(jobId), listProjects()]);
			// Init settings form
			settingsName = job.name;
			settingsSchedule = job.schedule;
			settingsImage = job.image;
			settingsCommand = parseCommand(job.command);
			settingsTimeout = job.timeout_seconds ?? null;
			settingsEnvVars = job.env ? { ...job.env } : {};
			settingsConcurrencyPolicy = job.concurrency_policy ?? 'skip';
			settingsMemoryMb = job.memory_mb ?? null;
			settingsCpuShares = job.cpu_shares ?? null;
			settingsTimezone = job.timezone ?? '';
			settingsMaxRetries = job.max_retries ?? 0;
			settingsRetryDelay = job.retry_delay_seconds ?? 60;
			settingsVolumes = job.volumes ? [...job.volumes] : [];
			settingsProjectId = job.project_id;
			setUiContext(cronJobContext(job));
		} catch (e: any) {
			error = e.message ?? 'Failed to load cron job';
		}
	}

	async function loadRuns(reset = true) {
		if (reset) {
			runsOffset = 0;
			runs = [];
		}
		runsLoading = true;
		runsError = '';
		try {
			const page = await listCronJobRuns(jobId, { limit: RUNS_PAGE_SIZE, offset: runsOffset });
			if (reset) {
				runs = page;
			} else {
				runs = [...runs, ...page];
			}
			runsHasMore = page.length === RUNS_PAGE_SIZE;
			runsOffset += page.length;
		} catch (e: any) {
			runsError = e.message ?? 'Failed to load run history';
		} finally {
			runsLoading = false;
		}
	}

	async function loadMoreRuns() {
		runsLoadingMore = true;
		try {
			const nextPage = await listCronJobRuns(jobId, { limit: RUNS_PAGE_SIZE, offset: runsOffset });
			runs = [...runs, ...nextPage];
			runsHasMore = nextPage.length === RUNS_PAGE_SIZE;
			runsOffset += nextPage.length;
		} catch (e: any) {
			runsError = e.message ?? 'Failed to load more runs';
		} finally {
			runsLoadingMore = false;
		}
	}

	async function handleTrigger() {
		triggering = true;
		triggerError = '';
		try {
			openFeedPanel();
			await triggerCronJob(jobId);
			await refreshJobData();
			await loadRuns();
		} catch (e: any) {
			triggerError = e.message ?? 'Failed to trigger job';
		} finally {
			triggering = false;
		}
	}

	async function handleToggleEnabled() {
		if (!job) return;
		try {
			job = await updateCronJob(jobId, { enabled: !job.enabled });
			setUiContext(cronJobContext(job));
		} catch (e: any) {
			error = e.message ?? 'Failed to update job';
		}
	}

	async function handleDelete() {
		try {
			await deleteCronJob(jobId);
			goto('/cron');
		} catch (e: any) {
			error = e.message ?? 'Failed to delete job';
		}
	}

	async function handleSaveSettings() {
		settingsSaving = true;
		settingsError = '';
		settingsSuccess = '';
		try {
			const commandArr = settingsCommand.trim()
				? (settingsCommand
						.trim()
						.match(/(?:[^\s"]+|"[^"]*")+/g)
						?.map((t) => t.replace(/^"|"$/g, '')) ?? [])
				: [];
			const timeoutNum = optionalInt(settingsTimeout);
			const memoryMb = optionalInt(settingsMemoryMb);
			const cpuShares = optionalInt(settingsCpuShares);
			const maxRetries = intOr(settingsMaxRetries, 0);
			const retryDelay = intOr(settingsRetryDelay, 60);

			job = await updateCronJob(jobId, {
				name: settingsName.trim(),
				project_id: settingsProjectId,
				schedule: settingsSchedule.trim(),
				image: settingsImage.trim(),
				// Send [] explicitly to clear command, or the parsed array
				command: commandArr.length > 0 ? commandArr : [],
				timeout_seconds: timeoutNum,
				env: settingsEnvVars,
				concurrency_policy: settingsConcurrencyPolicy,
				memory_mb: memoryMb,
				cpu_shares: cpuShares,
				timezone: settingsTimezone.trim() || null,
				max_retries: maxRetries,
				retry_delay_seconds: retryDelay,
				// Send name/app/container_path only: the server assigns host folders.
				volumes:
					settingsVolumes.length > 0
						? settingsVolumes.map((v) => ({
								container_path: v.container_path,
								...(v.name ? { name: v.name } : {}),
								...(v.app ? { app: v.app } : {})
							}))
						: null
			});
			setUiContext(cronJobContext(job));
			settingsSuccess = 'Settings saved.';
		} catch (e: any) {
			settingsError = e.message ?? 'Failed to save settings';
		} finally {
			settingsSaving = false;
		}
	}

	/** Parse stored command (JSON string or null) → shell-style string for the form */
	function parseCommand(raw: string | null): string {
		if (!raw) return '';
		try {
			const arr = JSON.parse(raw) as string[];
			return arr.map((t) => (t.includes(' ') ? `"${t}"` : t)).join(' ');
		} catch {
			return raw;
		}
	}

	function statusVariant(
		status: string
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		switch (status) {
			case 'success':
				return 'default';
			case 'failed':
				return 'destructive';
			case 'running':
				return 'secondary';
			default:
				return 'outline';
		}
	}

	function jobStatusVariant(
		j: CronJob
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		if (!j.enabled) return 'secondary';
		switch (j.status) {
			case 'idle':
				return 'outline';
			case 'running':
				return 'default';
			case 'failed':
				return 'destructive';
			default:
				return 'outline';
		}
	}

	function lastRunStatusVariant(
		s: string | null
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		if (s === 'success') return 'default';
		if (s === 'failed') return 'destructive';
		return 'outline';
	}

	function formatDuration(run: CronJobRun): string {
		if (!run.finished_at) return '—';
		const ms = new Date(run.finished_at).getTime() - new Date(run.started_at).getTime();
		if (ms < 1000) return `${ms}ms`;
		return `${(ms / 1000).toFixed(1)}s`;
	}

	// ── Env var dialog helpers ──

	function openEnvAdd() {
		envEditingKey = null;
		envNewKey = '';
		envNewValue = '';
		envDialogError = '';
		envDialogOpen = true;
	}

	function openEnvEdit(key: string, value: string) {
		envEditingKey = key;
		envNewKey = key;
		envNewValue = value;
		envDialogError = '';
		envDialogOpen = true;
	}

	function handleEnvConfirm() {
		const key = envNewKey.trim();
		if (!key) {
			envDialogError = 'Key is required';
			return;
		}
		if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
			envDialogError =
				'Key must start with a letter or underscore and contain only letters, numbers, and underscores';
			return;
		}
		if (envEditingKey !== null && envEditingKey !== key) {
			const next = { ...settingsEnvVars };
			delete next[envEditingKey];
			settingsEnvVars = { ...next, [key]: envNewValue };
		} else {
			settingsEnvVars = { ...settingsEnvVars, [key]: envNewValue };
		}
		envDialogOpen = false;
	}

	function handleEnvRemove(key: string) {
		const next = { ...settingsEnvVars };
		delete next[key];
		settingsEnvVars = next;
	}

	function handleEnvKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') handleEnvConfirm();
	}

	// ── Volume dialog helpers ──

	function openVolAdd() {
		volEditingIdx = null;
		volSource = OWN_FOLDER;
		volName = '';
		volContainerPath = '';
		volDialogError = '';
		volDialogOpen = true;
	}

	function openVolEdit(idx: number) {
		volEditingIdx = idx;
		volSource = settingsVolumes[idx].app || OWN_FOLDER;
		volName = settingsVolumes[idx].name ?? '';
		volContainerPath = settingsVolumes[idx].container_path;
		volDialogError = '';
		volDialogOpen = true;
	}

	function handleVolConfirm() {
		const name = volName.trim();
		const container = volContainerPath.trim();
		volDialogError = validateCronVolume(name, container);
		if (volDialogError) return;
		const entry: CronVolumeInput = {
			container_path: container,
			...(name ? { name } : {}),
			...(volSource !== OWN_FOLDER ? { app: volSource } : {})
		};
		if (volEditingIdx !== null) {
			settingsVolumes = settingsVolumes.map((v, i) => (i === volEditingIdx ? entry : v));
		} else {
			settingsVolumes = [...settingsVolumes, entry];
		}
		volDialogOpen = false;
	}

	function handleVolRemove(idx: number) {
		settingsVolumes = settingsVolumes.filter((_, i) => i !== idx);
	}

	$effect(() => {
		const routeData = data as CronJobDetailData;
		job = routeData.job;
		projects = routeData.projects;
		if (job) {
			settingsName = job.name;
			settingsSchedule = job.schedule;
			settingsImage = job.image;
			settingsCommand = parseCommand(job.command);
			settingsTimeout = job.timeout_seconds ?? null;
			settingsEnvVars = job.env ? { ...job.env } : {};
			settingsConcurrencyPolicy = job.concurrency_policy ?? 'skip';
			settingsMemoryMb = job.memory_mb ?? null;
			settingsCpuShares = job.cpu_shares ?? null;
			settingsTimezone = job.timezone ?? '';
			settingsMaxRetries = job.max_retries ?? 0;
			settingsRetryDelay = job.retry_delay_seconds ?? 60;
			settingsVolumes = job.volumes ? [...job.volumes] : [];
			settingsProjectId = job.project_id;
			setUiContext(cronJobContext(job));
		}
	});

	$effect(() => {
		return () => clearUiContext();
	});
</script>

{#if job}
	<div class="space-y-6">
		<!-- Breadcrumb -->
		<Breadcrumb items={[{ label: 'Cron Jobs', href: '/cron' }, { label: job.name }]} />

		<!-- Error banner -->
		{#if error}
			<ErrorAlert title="Error" description={error} />
		{/if}

		<!-- Header -->
		<div class="flex h-14 items-center justify-between">
			<div>
				<h2 class="text-3xl font-extrabold uppercase">{job.name}</h2>
				<div class="mt-2 flex items-center gap-3 text-sm text-muted-foreground">
					<Badge variant={jobStatusVariant(job)} class="capitalize">
						{job.enabled ? job.status : 'disabled'}
					</Badge>
					{#if job.last_run_status}
						<Badge variant={lastRunStatusVariant(job.last_run_status)} class="text-xs capitalize">
							last: {job.last_run_status}
						</Badge>
					{/if}
					<span class="font-mono text-xs">{job.schedule}</span>
					<span class="truncate font-mono text-xs">{job.image}</span>
				</div>
			</div>
			<div class="flex gap-2">
				<Button
					variant="outline"
					size="sm"
					onclick={handleToggleEnabled}
					class={job.enabled
						? 'border-destructive/50 text-destructive hover:border-destructive hover:text-destructive'
						: ''}
				>
					{job.enabled ? 'Disable' : 'Enable'}
				</Button>
				<Button variant="outline" size="sm" onclick={handleTrigger} disabled={triggering}>
					{#if triggering}
						<RotateCw class="mr-1 h-4 w-4 animate-spin" />
						Triggering...
					{:else}
						<Play class="mr-1 h-4 w-4" />
						Run Now
					{/if}
				</Button>
				<Button variant="destructive" size="sm" onclick={() => (showDeleteDialog = true)}>
					Delete
				</Button>
			</div>
		</div>

		{#if triggerError}
			<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive">{triggerError}</div>
		{/if}

		<Separator />

		<!-- Tabs -->
		<Tabs.Root
			value="overview"
			onValueChange={(v) => {
				if (v === 'runs') loadRuns();
			}}
		>
			<Tabs.List>
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="runs">Runs</Tabs.Trigger>
				<Tabs.Trigger value="settings">Settings</Tabs.Trigger>
			</Tabs.List>

			<!-- Overview tab -->
			<Tabs.Content value="overview">
				<div class="space-y-4 pt-4">
					<div class="grid gap-4 sm:grid-cols-2">
						<!-- Status -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Status
							</h4>
							<div class="space-y-1 text-sm">
								<div class="flex items-center gap-2">
									<span class="text-muted-foreground">State:</span>
									<Badge variant={jobStatusVariant(job)} class="text-xs capitalize">
										{job.enabled ? job.status : 'disabled'}
									</Badge>
								</div>
								{#if job.last_run_status}
									<div class="flex items-center gap-2">
										<span class="text-muted-foreground">Last Outcome:</span>
										<Badge
											variant={lastRunStatusVariant(job.last_run_status)}
											class="text-xs capitalize"
										>
											{job.last_run_status}
										</Badge>
									</div>
								{/if}
								<div>
									<span class="text-muted-foreground">Enabled:</span>
									{job.enabled ? 'Yes' : 'No'}
								</div>
								<div>
									<span class="text-muted-foreground">Concurrency:</span>
									<span class="font-mono text-xs">{job.concurrency_policy}</span>
								</div>
								<div>
									<span class="text-muted-foreground">Job ID:</span>
									<span class="font-mono text-xs">{job.id}</span>
								</div>
								<div>
									<span class="text-muted-foreground">Created:</span>
									{new Date(job.created_at).toLocaleString()}
								</div>
								<div>
									<span class="text-muted-foreground">Updated:</span>
									{new Date(job.updated_at).toLocaleString()}
								</div>
							</div>
						</div>

						<!-- Schedule -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Schedule
							</h4>
							<div class="space-y-1 text-sm">
								<div>
									<span class="text-muted-foreground">Cron:</span>
									<span class="font-mono text-xs">{job.schedule}</span>
								</div>
								{#if job.timezone}
									<div>
										<span class="text-muted-foreground">Timezone:</span>
										<span class="font-mono text-xs">{job.timezone}</span>
									</div>
								{/if}
								<div>
									<span class="text-muted-foreground">Last Run:</span>
									{job.last_run_at ? new Date(job.last_run_at).toLocaleString() : 'Never'}
								</div>
								<div>
									<span class="text-muted-foreground">Next Run:</span>
									{job.next_run_at && job.enabled
										? new Date(job.next_run_at).toLocaleString()
										: 'Not scheduled'}
								</div>
								{#if job.project_id}
									<div>
										<span class="text-muted-foreground">Project ID:</span>
										<span class="font-mono text-xs">{job.project_id}</span>
									</div>
								{/if}
							</div>
						</div>

						<!-- Container -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Container
							</h4>
							<div class="space-y-1 text-sm">
								<div class="truncate">
									<span class="text-muted-foreground">Image:</span>
									<span class="font-mono text-xs">{job.image}</span>
								</div>
								<div>
									<span class="text-muted-foreground">Command:</span>
									{#if job.command}
										<span class="font-mono text-xs">{parseCommand(job.command)}</span>
									{:else}
										<span class="text-muted-foreground">Default entrypoint</span>
									{/if}
								</div>
								<div>
									<span class="text-muted-foreground">Timeout:</span>
									{#if job.timeout_seconds != null}
										<span class="font-mono text-xs">{job.timeout_seconds}s</span>
									{:else}
										<span class="text-muted-foreground">None</span>
									{/if}
								</div>
								<div>
									<span class="text-muted-foreground">Memory:</span>
									{#if job.memory_mb != null}
										<span class="font-mono text-xs">{job.memory_mb} MiB</span>
									{:else}
										<span class="text-muted-foreground">No limit</span>
									{/if}
								</div>
								<div>
									<span class="text-muted-foreground">CPU shares:</span>
									{#if job.cpu_shares != null}
										<span class="font-mono text-xs">{job.cpu_shares}</span>
									{:else}
										<span class="text-muted-foreground">Default (1024)</span>
									{/if}
								</div>
								<div>
									<span class="text-muted-foreground">Env vars:</span>
									{#if job.env && Object.keys(job.env).length > 0}
										<span class="font-mono text-xs">{Object.keys(job.env).length} defined</span>
									{:else}
										<span class="text-muted-foreground">None</span>
									{/if}
								</div>
							</div>
						</div>

						<!-- Retry & Volumes -->
						<div class="space-y-3 rounded border border-dotted p-4">
							<h4
								class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
							>
								Retry & Volumes
							</h4>
							<div class="space-y-1 text-sm">
								<div>
									<span class="text-muted-foreground">Max retries:</span>
									<span class="font-mono text-xs">{job.max_retries}</span>
								</div>
								{#if job.max_retries > 0}
									<div>
										<span class="text-muted-foreground">Retry delay:</span>
										<span class="font-mono text-xs">{job.retry_delay_seconds}s</span>
									</div>
								{/if}
								<div>
									<span class="text-muted-foreground">Volumes:</span>
									{#if job.volumes && job.volumes.length > 0}
										<ul class="mt-1 space-y-0.5">
											{#each job.volumes as vol, vi (vi)}
												<li class="font-mono text-xs text-muted-foreground">
													{vol.host_path} → {vol.container_path}
												</li>
											{/each}
										</ul>
									{:else}
										<span class="text-muted-foreground">None</span>
									{/if}
								</div>
							</div>
						</div>
					</div>
				</div>
			</Tabs.Content>

			<!-- Runs tab -->
			<Tabs.Content value="runs">
				<div class="space-y-4 pt-4">
					<div class="flex items-center justify-between">
						<h3 class="text-sm font-medium">Run History</h3>
						<Button variant="outline" size="sm" onclick={() => loadRuns()} disabled={runsLoading}>
							{#if runsLoading}
								<RotateCw class="mr-1 h-3 w-3 animate-spin" />
							{/if}
							Refresh
						</Button>
					</div>

					{#if runsError}
						<ErrorAlert title="Error" description={runsError} />
					{/if}

					{#if runsLoading && runs.length === 0}
						<div class="py-8 text-center text-sm text-muted-foreground">Loading...</div>
					{:else if runs.length === 0}
						<div class="rounded-md border py-12 text-center text-sm text-muted-foreground">
							No runs yet. Use "Run Now" to trigger a manual execution.
						</div>
					{:else}
						<div class="divide-y rounded-md border">
							{#each runs as run (run.id)}
								{@const isExpanded = expandedRunId === run.id}
								<div>
									<!-- Run row -->
									<button
										class="flex w-full items-center gap-3 px-4 py-2.5 text-left transition-colors hover:bg-muted/40"
										onclick={() => (expandedRunId = isExpanded ? null : run.id)}
									>
										{#if isExpanded}
											<ChevronDown class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
										{:else}
											<ChevronRight class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
										{/if}
										<Badge variant={statusVariant(run.status)} class="w-16 justify-center text-xs">
											{run.status}
										</Badge>
										{#if run.attempt > 1}
											<span class="font-mono text-[10px] text-muted-foreground/60 uppercase">
												attempt {run.attempt}
											</span>
										{/if}
										<span class="flex-1 font-mono text-xs text-muted-foreground">
											{new Date(run.started_at).toLocaleString()}
										</span>
										<span class="font-mono text-xs text-muted-foreground">
											{formatDuration(run)}
										</span>
										{#if run.exit_code !== null}
											<span class="font-mono text-xs text-muted-foreground">
												exit {run.exit_code}
											</span>
										{/if}
									</button>

									<!-- Expanded output -->
									{#if isExpanded && run.output}
										<div class="bg-muted/20 px-4 pt-1 pb-3">
											<pre
												class="max-h-64 overflow-y-auto rounded-sm bg-muted p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap">{run.output}</pre>
										</div>
									{:else if isExpanded}
										<div class="bg-muted/20 px-4 pt-1 pb-3">
											<p class="text-xs text-muted-foreground">No output captured.</p>
										</div>
									{/if}
								</div>
							{/each}
						</div>

						<!-- Load More -->
						{#if runsHasMore}
							<div class="flex justify-center pt-2">
								<Button
									variant="outline"
									size="sm"
									onclick={loadMoreRuns}
									disabled={runsLoadingMore}
								>
									{#if runsLoadingMore}
										<RotateCw class="mr-1 h-3 w-3 animate-spin" />
									{/if}
									Load More
								</Button>
							</div>
						{/if}
					{/if}
				</div>
			</Tabs.Content>

			<!-- Settings tab -->
			<Tabs.Content value="settings">
				<div class="space-y-6 pt-4">
					{#if settingsError}
						<ErrorAlert title="Error" description={settingsError} />
					{/if}
					{#if !isAdmin()}
						<div class="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
							Only admins can change cron job settings, because a job runs any image and command on
							this server.
						</div>
					{/if}
					{#if settingsSuccess}
						<div class="rounded-md bg-success/10 p-3 text-sm text-success">{settingsSuccess}</div>
					{/if}

					<div class="grid max-w-xl gap-4">
						<!-- Name -->
						<div class="grid gap-2">
							<Label for="settings-name">Name</Label>
							<Input
								id="settings-name"
								type="text"
								bind:value={settingsName}
								placeholder="nightly-backup"
							/>
							<p class="text-xs text-muted-foreground">
								Rename this job. Must be unique. Lowercase, no spaces.
							</p>
						</div>

						<!-- Project -->
						<div class="grid gap-2">
							<Label>Project</Label>
							<Select.Root
								type="single"
								value={settingsProjectId ?? '__none__'}
								onValueChange={(v) => {
									settingsProjectId = v === '__none__' ? null : v;
								}}
							>
								<Select.Trigger class="w-full">
									{#if settingsProjectId}
										{projects.find((p) => p.id === settingsProjectId)?.name ?? settingsProjectId}
									{:else}
										No project
									{/if}
								</Select.Trigger>
								<Select.Content>
									<Select.Item value="__none__">No project</Select.Item>
									{#each projects as proj (proj.id)}
										<Select.Item value={proj.id}>{proj.name}</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
							<p class="text-xs text-muted-foreground">Associate with a project (optional).</p>
						</div>

						<div class="grid gap-2">
							<Label for="settings-schedule">Schedule (cron expression)</Label>
							<Input
								id="settings-schedule"
								type="text"
								bind:value={settingsSchedule}
								placeholder="0 * * * *"
								class="font-mono"
							/>
							<p class="text-xs text-muted-foreground">
								5-field cron expression. e.g. <span class="font-mono">0 * * * *</span> = every hour.
							</p>
						</div>

						<!-- Timezone -->
						<div class="grid gap-2">
							<Label for="settings-timezone">
								Timezone <span class="text-muted-foreground">(optional)</span>
							</Label>
							<Input
								id="settings-timezone"
								type="text"
								bind:value={settingsTimezone}
								placeholder="America/New_York"
								class="font-mono"
							/>
							<p class="text-xs text-muted-foreground">
								IANA timezone for schedule evaluation. Leave empty to use UTC.
							</p>
						</div>

						<div class="grid gap-2">
							<Label for="settings-image">Docker Image</Label>
							<Input
								id="settings-image"
								type="text"
								bind:value={settingsImage}
								placeholder="alpine:latest"
								class="font-mono"
							/>
						</div>

						<div class="grid gap-2">
							<Label for="settings-command">Command (optional)</Label>
							<Input
								id="settings-command"
								type="text"
								bind:value={settingsCommand}
								placeholder="sh -c &quot;echo hello&quot;"
								class="font-mono"
							/>
							<p class="text-xs text-muted-foreground">
								Space-separated arguments. Leave empty to clear to the image default entrypoint.
							</p>
						</div>

						<!-- Timeout -->
						<div class="grid gap-2">
							<Label for="settings-timeout">
								Timeout <span class="text-muted-foreground">(optional, seconds)</span>
							</Label>
							<Input
								id="settings-timeout"
								type="number"
								min="1"
								bind:value={settingsTimeout}
								placeholder="300"
								class="font-mono"
							/>
							<p class="text-xs text-muted-foreground">
								Maximum time the container is allowed to run. Leave empty to remove the timeout.
							</p>
						</div>

						<!-- Concurrency Policy -->
						<div class="grid gap-2">
							<Label>Concurrency Policy</Label>
							<Select.Root
								type="single"
								value={settingsConcurrencyPolicy}
								onValueChange={(v) => {
									if (v) settingsConcurrencyPolicy = v;
								}}
							>
								<Select.Trigger class="w-full">
									{settingsConcurrencyPolicy === 'allow'
										? 'Allow concurrent runs'
										: 'Skip if already running'}
								</Select.Trigger>
								<Select.Content>
									<Select.Item value="skip">Skip if already running</Select.Item>
									<Select.Item value="allow">Allow concurrent runs</Select.Item>
								</Select.Content>
							</Select.Root>
							<p class="text-xs text-muted-foreground">
								Whether to skip or allow a new run when the previous one is still running.
							</p>
						</div>

						<!-- Resource limits -->
						<div class="grid grid-cols-2 gap-4">
							<div class="grid gap-2">
								<Label for="settings-memory">
									Memory <span class="text-muted-foreground">(MiB)</span>
								</Label>
								<Input
									id="settings-memory"
									type="number"
									min="4"
									bind:value={settingsMemoryMb}
									placeholder="128"
									class="font-mono"
								/>
								<p class="text-xs text-muted-foreground">Leave empty for no limit.</p>
							</div>
							<div class="grid gap-2">
								<Label for="settings-cpu">CPU Shares</Label>
								<Input
									id="settings-cpu"
									type="number"
									min="2"
									bind:value={settingsCpuShares}
									placeholder="1024"
									class="font-mono"
								/>
								<p class="text-xs text-muted-foreground">Relative weight. Default = 1024.</p>
							</div>
						</div>

						<!-- Retries -->
						<div class="grid grid-cols-2 gap-4">
							<div class="grid gap-2">
								<Label for="settings-max-retries">Max Retries</Label>
								<Input
									id="settings-max-retries"
									type="number"
									min="0"
									max="10"
									bind:value={settingsMaxRetries}
									placeholder="0"
									class="font-mono"
								/>
								<p class="text-xs text-muted-foreground">0 = disabled.</p>
							</div>
							<div class="grid gap-2">
								<Label for="settings-retry-delay">
									Retry Delay <span class="text-muted-foreground">(seconds)</span>
								</Label>
								<Input
									id="settings-retry-delay"
									type="number"
									min="1"
									bind:value={settingsRetryDelay}
									placeholder="60"
									class="font-mono"
								/>
							</div>
						</div>

						<!-- Environment Variables -->
						<div class="grid gap-2">
							<div class="flex items-center justify-between">
								<Label>Environment Variables</Label>
								<Dialog.Root bind:open={envDialogOpen}>
									<Dialog.Trigger>
										{#snippet child({ props })}
											<Button variant="outline" size="sm" onclick={openEnvAdd} {...props}>
												<Plus class="mr-1 h-3.5 w-3.5" />
												Add Variable
											</Button>
										{/snippet}
									</Dialog.Trigger>
									<Dialog.Content class="sm:max-w-md">
										<Dialog.Header>
											<Dialog.Title>
												{envEditingKey !== null ? 'Edit' : 'Add'} Environment Variable
											</Dialog.Title>
											<Dialog.Description>
												{envEditingKey !== null
													? 'Update the key or value for this environment variable.'
													: 'Enter a key and value for the new environment variable.'}
											</Dialog.Description>
										</Dialog.Header>
										<div class="space-y-4 py-2">
											<div class="space-y-2">
												<Label for="env-key">Key</Label>
												<Input
													id="env-key"
													bind:value={envNewKey}
													placeholder="DATABASE_URL"
													class="font-mono"
													onkeydown={handleEnvKeydown}
													autofocus
												/>
											</div>
											<div class="space-y-2">
												<Label for="env-value">Value</Label>
												<Input
													id="env-value"
													bind:value={envNewValue}
													placeholder="postgres://user:pass@host/db"
													class="font-mono"
													onkeydown={handleEnvKeydown}
												/>
											</div>
											{#if envDialogError}
												<p class="text-xs text-destructive">{envDialogError}</p>
											{/if}
										</div>
										<Dialog.Footer>
											<Button variant="outline" onclick={() => (envDialogOpen = false)}>
												Cancel
											</Button>
											<Button onclick={handleEnvConfirm}>
												{envEditingKey !== null ? 'Save' : 'Add'}
											</Button>
										</Dialog.Footer>
									</Dialog.Content>
								</Dialog.Root>
							</div>

							{#if envEntries.length === 0}
								<div class="rounded border border-dashed border-border py-6 text-center">
									<p class="text-xs text-muted-foreground">No environment variables defined.</p>
								</div>
							{:else}
								<div class="overflow-hidden rounded border border-border">
									<table class="w-full text-sm">
										<thead>
											<tr class="border-b border-border bg-muted/50">
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Key</th
												>
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Value</th
												>
												<th class="w-16 px-3 py-2"></th>
											</tr>
										</thead>
										<tbody>
											{#each envEntries as [key, value], i (key)}
												<tr class={i < envEntries.length - 1 ? 'border-b border-border' : ''}>
													<td class="px-3 py-2 font-mono text-xs font-medium">{key}</td>
													<td
														class="max-w-0 truncate px-3 py-2 font-mono text-xs text-muted-foreground"
													>
														{#if !value}
															<span class="italic">empty</span>
														{:else}
															<span class="tracking-widest">••••••••</span>
														{/if}
													</td>
													<td class="px-3 py-2">
														<div class="flex items-center justify-end gap-1">
															<Button
																variant="ghost"
																size="icon"
																class="h-7 w-7 text-muted-foreground hover:text-foreground"
																aria-label="Edit variable"
																onclick={() => openEnvEdit(key, value)}
															>
																<Plus class="h-3.5 w-3.5 rotate-45" />
															</Button>
															<Button
																variant="ghost"
																size="icon"
																class="h-7 w-7 text-muted-foreground hover:text-destructive"
																aria-label="Delete variable"
																onclick={() => handleEnvRemove(key)}
															>
																<Trash2 class="h-3.5 w-3.5" />
															</Button>
														</div>
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								</div>
							{/if}
							<p class="text-xs text-muted-foreground">
								Values are write-only. Saving replaces all variables.
							</p>
						</div>

						<!-- Volumes -->
						<div class="grid gap-2">
							<div class="flex items-center justify-between">
								<Label>Volume Mounts</Label>
								<Dialog.Root bind:open={volDialogOpen}>
									<Dialog.Trigger>
										{#snippet child({ props })}
											<Button variant="outline" size="sm" onclick={openVolAdd} {...props}>
												<Plus class="mr-1 h-3.5 w-3.5" />
												Add Volume
											</Button>
										{/snippet}
									</Dialog.Trigger>
									<Dialog.Content class="sm:max-w-md">
										<Dialog.Header>
											<Dialog.Title>
												{volEditingIdx !== null ? 'Edit' : 'Add'} Volume Mount
											</Dialog.Title>
											<Dialog.Description
												>Mount a persistent folder into the job's container.</Dialog.Description
											>
										</Dialog.Header>
										<CronVolumeFields
											bind:source={volSource}
											bind:name={volName}
											bind:containerPath={volContainerPath}
										/>
										{#if volDialogError}
											<p class="text-xs text-destructive">{volDialogError}</p>
										{/if}
										<Dialog.Footer>
											<Button variant="outline" onclick={() => (volDialogOpen = false)}>
												Cancel
											</Button>
											<Button onclick={handleVolConfirm}>
												{volEditingIdx !== null ? 'Save' : 'Add'}
											</Button>
										</Dialog.Footer>
									</Dialog.Content>
								</Dialog.Root>
							</div>

							{#if settingsVolumes.length === 0}
								<div class="rounded border border-dashed border-border py-6 text-center">
									<p class="text-xs text-muted-foreground">No volume mounts defined.</p>
								</div>
							{:else}
								<div class="overflow-hidden rounded border border-border">
									<table class="w-full text-sm">
										<thead>
											<tr class="border-b border-border bg-muted/50">
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Stored in</th
												>
												<th
													class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
													>Container Path</th
												>
												<th class="w-16 px-3 py-2"></th>
											</tr>
										</thead>
										<tbody>
											{#each settingsVolumes as vol, i (i)}
												<tr class={i < settingsVolumes.length - 1 ? 'border-b border-border' : ''}>
													<td class="px-3 py-2 font-mono text-xs">{cronVolumeLabel(vol)}</td>
													<td class="px-3 py-2 font-mono text-xs text-muted-foreground">
														{vol.container_path}
													</td>
													<td class="px-3 py-2">
														<div class="flex items-center justify-end gap-1">
															<Button
																variant="ghost"
																size="icon"
																class="h-7 w-7 text-muted-foreground hover:text-foreground"
																aria-label="Edit volume"
																onclick={() => openVolEdit(i)}
															>
																<Plus class="h-3.5 w-3.5 rotate-45" />
															</Button>
															<Button
																variant="ghost"
																size="icon"
																class="h-7 w-7 text-muted-foreground hover:text-destructive"
																aria-label="Remove volume"
																onclick={() => handleVolRemove(i)}
															>
																<Trash2 class="h-3.5 w-3.5" />
															</Button>
														</div>
													</td>
												</tr>
											{/each}
										</tbody>
									</table>
								</div>
							{/if}
						</div>
					</div>

					<div class="flex gap-2">
						<Button onclick={handleSaveSettings} disabled={settingsSaving || !isAdmin()}>
							{settingsSaving ? 'Saving...' : 'Save Settings'}
						</Button>
					</div>

					<Separator />

					<!-- Danger zone -->
					<div class="grid gap-3">
						<h3 class="text-sm font-medium text-destructive">Danger Zone</h3>
						<div
							class="flex items-center justify-between rounded-md border border-destructive/30 p-4"
						>
							<div>
								<p class="text-sm font-medium">Delete this cron job</p>
								<p class="text-xs text-muted-foreground">
									This cannot be undone. The job and all run history will be permanently removed.
								</p>
							</div>
							<Button variant="destructive" size="sm" onclick={() => (showDeleteDialog = true)}>
								Delete
							</Button>
						</div>
					</div>
				</div>
			</Tabs.Content>
		</Tabs.Root>
	</div>
{:else}
	<div class="py-12 text-center text-muted-foreground">Cron job not found</div>
{/if}

<!-- Delete confirmation dialog -->
<ConfirmDeleteDialog
	bind:open={showDeleteDialog}
	title={`Delete "${job?.name}"?`}
	description="This cannot be undone. The cron job and all run history will be permanently removed."
	confirmation={job?.name ?? ''}
	onconfirm={handleDelete}
/>
