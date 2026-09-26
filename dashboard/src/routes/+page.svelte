<script lang="ts">
	import {
		getOnboardingStatus,
		getSystemMetrics,
		listActiveAlerts,
		type ActiveAlert,
		type Project,
		type App,
		type ManagedDatabase,
		type CronJob,
		type AuditLogEntry,
		type OnboardingStatus,
		type SystemMetrics
	} from '$lib/api';
	import { getUser, can } from '$lib/auth.svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import TrafficOverview from '$lib/components/metrics/traffic-overview.svelte';
	import { errorMessage } from '$lib/action.svelte';
	import { buildChecklist } from '$lib/onboarding';
	import FiringBanner from '$lib/components/alerts/FiringBanner.svelte';
	import { appTabHref } from '$lib/tabs';
	import { formatRelativePast } from '$lib/time';
	import Plus from '~icons/ri/add-line';
	import CheckIcon from '~icons/ri/checkbox-circle-fill';
	import TodoIcon from '~icons/ri/checkbox-blank-circle-line';
	import Rocket from '~icons/ri/rocket-line';
	import Database from '~icons/ri/database-2-line';
	import Clock from '~icons/ri/time-line';
	import { actorLabel } from '$lib/access';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let projects = $derived(data.projects as Project[]);
	let apps = $derived(data.apps as App[]);
	let databases = $derived(data.databases as ManagedDatabase[]);
	let cronJobs = $derived(data.cronJobs as CronJob[]);
	let recentActivity = $derived(data.recentActivity as AuditLogEntry[]);
	let serverName = $derived(data.serverName);

	let runningApps = $derived(apps.filter((a) => a.status === 'running').length);
	let troubledApps = $derived(
		apps.filter((a) => a.status === 'failed' || a.status === 'unhealthy')
	);
	let runningDatabases = $derived(databases.filter((d) => d.status === 'running').length);
	let failedJobs = $derived(cronJobs.filter((c) => c.enabled && c.last_run_status === 'failed'));

	// The checklist and health tile load in the background so the page shows at once
	// (the onboarding check resolves DNS, which can take a few seconds).
	let onboarding = $state<OnboardingStatus | null>(null);
	let onboardingError = $state('');
	let metrics = $state<SystemMetrics | null>(null);
	let firing = $state<ActiveAlert[]>([]);

	$effect(() => {
		let cancelled = false;
		getOnboardingStatus()
			.then((s) => !cancelled && (onboarding = s))
			.catch((e) => !cancelled && (onboardingError = errorMessage(e)));
		getSystemMetrics()
			.then((m) => !cancelled && (metrics = m))
			.catch(() => {});
		if (can('metrics:read')) {
			listActiveAlerts()
				.then((a) => !cancelled && (firing = a.filter((x) => x.state === 'firing')))
				.catch(() => {});
		}
		return () => {
			cancelled = true;
		};
	});

	let checklist = $derived(
		onboarding ? buildChecklist(onboarding, getUser()?.totp_enabled ?? false) : []
	);
	let doneCount = $derived(checklist.filter((i) => i.done).length);
	let checklistComplete = $derived(checklist.length > 0 && doneCount === checklist.length);

	function projectName(id: string | null): string | null {
		if (!id) return null;
		return projects.find((p) => p.id === id)?.name ?? null;
	}

	function actionLabel(action: string): string {
		return action.replace(/_/g, ' ');
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader title="Home" docTitle="Home" description={serverName || undefined}>
		{#snippet actions()}
			{#if can('apps:write')}
				<Button variant="outline" size="sm" href="/projects">
					<Plus class="mr-1 h-4 w-4" />
					New project
				</Button>
				<Button size="sm" href="/apps/new">
					<Plus class="mr-1 h-4 w-4" />
					New app
				</Button>
			{/if}
		{/snippet}
	</PageHeader>

	<FiringBanner alerts={firing} />

	<!-- First-run checklist -->
	{#if onboardingError}
		<InlineError title="Couldn't load the setup checklist" message={onboardingError} />
	{:else if !onboarding}
		<Skeleton class="h-40 w-full" />
	{:else if !checklistComplete}
		<section
			aria-labelledby="checklist-heading"
			class="rounded-lg border bg-card p-4 sm:p-5"
			data-testid="setup-checklist"
		>
			<div class="flex flex-wrap items-baseline justify-between gap-2">
				<h2 id="checklist-heading" class="text-base font-semibold">Finish setting up</h2>
				<p class="text-sm text-muted-foreground">{doneCount} of {checklist.length} done</p>
			</div>
			<div
				class="mt-3 h-1.5 overflow-hidden rounded-full bg-muted"
				role="progressbar"
				aria-label="Setup progress"
				aria-valuemin={0}
				aria-valuemax={checklist.length}
				aria-valuenow={doneCount}
			>
				<div class="h-full bg-success" style="width: {(doneCount / checklist.length) * 100}%"></div>
			</div>
			<ol class="mt-4 divide-y">
				{#each checklist as item (item.id)}
					<li class="flex flex-col gap-2 py-3 sm:flex-row sm:items-center sm:justify-between">
						<div class="flex min-w-0 items-start gap-3">
							{#if item.done}
								<CheckIcon aria-hidden="true" class="mt-0.5 size-5 shrink-0 text-success" />
							{:else}
								<TodoIcon aria-hidden="true" class="mt-0.5 size-5 shrink-0 text-muted-foreground" />
							{/if}
							<div class="min-w-0">
								<p class="text-sm font-medium">
									{item.title}
									<span class="sr-only">{item.done ? '(done)' : '(to do)'}</span>
								</p>
								<p class="text-sm text-muted-foreground">{item.detail}</p>
							</div>
						</div>
						{#if !item.done}
							<Button
								variant="outline"
								size="sm"
								href={item.href}
								class="shrink-0 self-start sm:self-auto"
							>
								{item.action}
							</Button>
						{/if}
					</li>
				{/each}
			</ol>
		</section>
	{/if}

	<!-- Health tiles -->
	<section aria-label="Overview" class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
		<a
			href="/apps"
			class="rounded-lg border bg-card p-4 transition-colors hover:border-foreground/40"
		>
			<p class="flex items-center gap-2 text-sm text-muted-foreground">
				<Rocket aria-hidden="true" class="size-4" /> Apps
			</p>
			<p class="mt-2 text-2xl font-semibold">
				{runningApps}<span class="text-base text-muted-foreground"> / {apps.length} running</span>
			</p>
			{#if troubledApps.length}
				<p class="mt-1 text-sm text-destructive">
					{troubledApps.length} need{troubledApps.length === 1 ? 's' : ''} attention
				</p>
			{:else}
				<p class="mt-1 text-sm text-muted-foreground">No failing apps</p>
			{/if}
		</a>

		<a
			href="/databases"
			class="rounded-lg border bg-card p-4 transition-colors hover:border-foreground/40"
		>
			<p class="flex items-center gap-2 text-sm text-muted-foreground">
				<Database aria-hidden="true" class="size-4" /> Databases
			</p>
			<p class="mt-2 text-2xl font-semibold">
				{runningDatabases}<span class="text-base text-muted-foreground">
					/ {databases.length} running</span
				>
			</p>
			<p class="mt-1 text-sm text-muted-foreground">
				{cronJobs.length} cron job{cronJobs.length === 1 ? '' : 's'}{failedJobs.length
					? `, ${failedJobs.length} failed last run`
					: ''}
			</p>
		</a>

		<div class="rounded-lg border bg-card p-4" data-testid="last-deploy-tile">
			<p class="flex items-center gap-2 text-sm text-muted-foreground">
				<Clock aria-hidden="true" class="size-4" /> Last deploy
			</p>
			{#if !onboarding}
				<Skeleton class="mt-3 h-6 w-3/4" />
			{:else if onboarding.last_deployment}
				{@const dep = onboarding.last_deployment}
				<a
					href={appTabHref(dep.app_id, 'deployments')}
					class="mt-2 block truncate text-lg font-semibold hover:underline">{dep.app_name}</a
				>
				<p class="mt-1 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
					<StatusBadge status={dep.status} kind="deployment" size="sm" />
					{formatRelativePast(dep.finished_at ?? dep.created_at)}
				</p>
			{:else}
				<p class="mt-2 text-sm text-muted-foreground">Nothing deployed yet.</p>
			{/if}
		</div>

		<a
			href="/system"
			class="rounded-lg border bg-card p-4 transition-colors hover:border-foreground/40"
		>
			<p class="text-sm text-muted-foreground">Server health</p>
			{#if metrics}
				<dl class="mt-2 grid grid-cols-3 gap-2 text-sm">
					<div>
						<dt class="text-muted-foreground">CPU</dt>
						<dd class="text-lg font-semibold">{Math.round(metrics.cpu_percent)}%</dd>
					</div>
					<div>
						<dt class="text-muted-foreground">Memory</dt>
						<dd class="text-lg font-semibold">{Math.round(metrics.mem_percent)}%</dd>
					</div>
					<div>
						<dt class="text-muted-foreground">Disk</dt>
						<dd
							class={metrics.disk_percent >= 90
								? 'text-lg font-semibold text-destructive'
								: 'text-lg font-semibold'}
						>
							{Math.round(metrics.disk_percent)}%
						</dd>
					</div>
				</dl>
			{:else}
				<Skeleton class="mt-3 h-6 w-full" />
			{/if}
		</a>
	</section>

	{#if can('metrics:read')}
		<TrafficOverview />
	{/if}

	<div class="grid gap-4 lg:grid-cols-3">
		<!-- Apps that need attention, then everything by project -->
		<section aria-labelledby="apps-heading" class="rounded-lg border bg-card p-4 lg:col-span-2">
			<h2 id="apps-heading" class="text-base font-semibold">Apps</h2>
			{#if apps.length === 0}
				<p class="mt-2 text-sm text-muted-foreground">
					No apps yet. <a href="/apps/new" class="underline underline-offset-2"
						>Deploy your first app</a
					>.
				</p>
			{:else}
				<ul class="mt-3 divide-y">
					{#each [...troubledApps, ...apps.filter((a) => !troubledApps.includes(a))].slice(0, 8) as app (app.id)}
						<li class="flex items-center justify-between gap-3 py-2">
							<div class="min-w-0">
								<a
									href={`/apps/${app.id}`}
									class="block truncate text-sm font-medium hover:underline">{app.name}</a
								>
								<p class="truncate text-xs text-muted-foreground">
									{projectName(app.project_id) ?? 'No project'}{app.domain
										? ` · ${app.domain}`
										: ''}
								</p>
							</div>
							<StatusBadge status={app.status} kind="app" size="sm" />
						</li>
					{/each}
				</ul>
				{#if apps.length > 8}
					<a
						href="/apps"
						class="mt-2 inline-block text-sm text-muted-foreground hover:text-foreground"
					>
						All {apps.length} apps →
					</a>
				{/if}
			{/if}
		</section>

		<section aria-labelledby="activity-heading" class="rounded-lg border bg-card p-4">
			<div class="flex items-center justify-between">
				<h2 id="activity-heading" class="text-base font-semibold">Recent activity</h2>
				<a href="/activity" class="text-sm text-muted-foreground hover:text-foreground"
					>View all →</a
				>
			</div>
			{#if recentActivity.length === 0}
				<p class="mt-2 text-sm text-muted-foreground">Nothing yet.</p>
			{:else}
				<ul class="mt-3 space-y-2">
					{#each recentActivity.slice(0, 5) as entry (entry.seq)}
						<li class="text-sm">
							<span class="font-medium">{actionLabel(entry.action)}</span>
							<span class="text-muted-foreground">
								{entry.resource_name ?? entry.detail ?? 'System'} · {actorLabel(entry)} ·
								{formatRelativePast(entry.created_at)}
							</span>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	</div>
</div>
