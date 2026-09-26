<script lang="ts">
	import {
		type Project,
		type App,
		type ManagedDatabase,
		type CronJob,
		type AuditLogEntry
	} from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { goto } from '$app/navigation';
	import Plus from '~icons/ri/add-line';
	import Rocket from '~icons/ri/rocket-line';
	import Database from '~icons/ri/database-2-line';
	import Clock from '~icons/ri/time-line';
	import Activity from '~icons/ri/pulse-line';
	import Zap from '~icons/ri/flashlight-line';
	import Server from '~icons/ri/server-line';
	import Box from '~icons/ri/box-3-line';
	import { cn } from '$lib/utils';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let projects = $derived(data.projects as Project[]);
	let apps = $derived(data.apps as App[]);
	let databases = $derived(data.databases as ManagedDatabase[]);
	let cronJobs = $derived(data.cronJobs as CronJob[]);
	let recentActivity = $derived(data.recentActivity as AuditLogEntry[]);
	let serverName = $derived(data.serverName);

	let runningApps = $derived(apps.filter((a) => a.status === 'running').length);
	let runningDatabases = $derived(databases.filter((d) => d.status === 'running').length);
	let enabledCronJobs = $derived(cronJobs.filter((c) => c.enabled).length);
	let hasResources = $derived(
		projects.length > 0 || apps.length > 0 || databases.length > 0 || cronJobs.length > 0
	);

	// Group apps by project
	let appsByProject = $derived(
		apps.reduce<Record<string, App[]>>((acc, app) => {
			const pid = app.project_id ?? 'unassigned';
			if (!acc[pid]) acc[pid] = [];
			acc[pid].push(app);
			return acc;
		}, {})
	);

	// Group databases by project
	let dbsByProject = $derived(
		databases.reduce<Record<string, ManagedDatabase[]>>((acc, db) => {
			const pid = db.project_id ?? 'unassigned';
			if (!acc[pid]) acc[pid] = [];
			acc[pid].push(db);
			return acc;
		}, {})
	);

	// Group cron jobs by project
	let jobsByProject = $derived(
		cronJobs.reduce<Record<string, CronJob[]>>((acc, job) => {
			const pid = job.project_id ?? 'unassigned';
			if (!acc[pid]) acc[pid] = [];
			acc[pid].push(job);
			return acc;
		}, {})
	);

	function actionVariant(
		action: string
	): 'default' | 'secondary' | 'destructive' | 'outline' | undefined {
		switch (action) {
			case 'delete':
				return 'destructive';
			case 'create':
			case 'deploy':
				return 'default';
			case 'stop':
				return 'secondary';
			default:
				return 'outline';
		}
	}

	function appStatusColor(app: App): string {
		switch (app.status) {
			case 'running':
				return 'border-success/60 bg-success/10';
			case 'failed':
				return 'border-destructive/60 bg-destructive/10';
			case 'unhealthy':
				return 'border-warn/60 bg-warn/10';
			default:
				return 'border-border bg-muted/50';
		}
	}

	function dbStatusColor(db: ManagedDatabase): string {
		return db.status === 'running' ? 'border-accent/60 bg-accent/10' : 'border-border bg-muted/20';
	}

	function jobStatusColor(job: CronJob): string {
		return job.enabled ? 'border-secondary/60 bg-secondary/10' : 'border-border bg-muted/20';
	}

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex items-center justify-between gap-4">
		<div>
			<h1 class="text-3xl font-extrabold uppercase">Dashboard</h1>
		</div>
		<div class="flex items-center gap-2">
			<Button variant="outline" size="sm" onclick={() => goto('/projects')}>
				<Plus class="mr-1 h-4 w-4" />
				New Project
			</Button>
		</div>
	</div>

	<!-- Tactical Server Map -->
	<div class="relative overflow-hidden rounded-lg border border-border bg-card">
		<!-- Grid background with scanlines -->
		<div
			class="pointer-events-none absolute inset-0 opacity-5"
			style="background-image: 
					linear-gradient(to right, var(--color-border) 1px, transparent 1px),
					linear-gradient(to bottom, var(--color-border) 1px, transparent 1px);
				background-size: 40px 40px;"
		></div>
		<div
			class="pointer-events-none absolute inset-0 opacity-5"
			style="background: repeating-linear-gradient(0deg, transparent, transparent 2px, var(--color-foreground) 2px, var(--color-foreground) 4px);"
		></div>

		<!-- Decorative corner markers -->
		<div
			class="pointer-events-none absolute top-0 left-0 h-8 w-8 border-t-2 border-l-2 border-border/70"
		></div>
		<div
			class="pointer-events-none absolute top-0 right-0 h-8 w-8 border-t-2 border-r-2 border-border/70"
		></div>
		<div
			class="pointer-events-none absolute bottom-0 left-0 h-8 w-8 border-b-2 border-l-2 border-border/70"
		></div>
		<div
			class="pointer-events-none absolute right-0 bottom-0 h-8 w-8 border-r-2 border-b-2 border-border/70"
		></div>

		<div class="relative p-6">
			<!-- Map Header -->
			<div class="mb-6 flex items-center justify-between border-b border-border pb-4">
				<div class="flex items-center gap-3">
					<Server class="h-5 w-5 text-muted-foreground" />
					<div>
						<p class="font-mono text-[10px] tracking-[0.3em] text-muted-foreground uppercase">
							INFRASTRUCTURE_MAP
						</p>
						<p class="font-mono text-sm tracking-wider text-foreground uppercase">
							{serverName || 'TOND SERVER'}
						</p>
					</div>
				</div>
				<div class="flex items-center gap-4 text-xs text-muted-foreground">
					<div class="flex items-center gap-2">
						<span class="inline-block size-2 rounded-full bg-success/60"></span>
						<span class="font-mono">{runningApps} APPS RUNNING</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="inline-block size-2 rounded-full bg-accent/60"></span>
						<span class="font-mono">{runningDatabases} DBS ACTIVE</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="inline-block size-2 rounded-full bg-secondary/60"></span>
						<span class="font-mono">{enabledCronJobs} CRON ENABLED</span>
					</div>
				</div>
			</div>

			{#if !hasResources}
				<div class="flex flex-col items-center justify-center py-16">
					<Box class="mb-4 h-12 w-12 text-muted-foreground" />
					<p class="font-mono text-[11px] tracking-widest text-muted-foreground uppercase">
						NO_ACTIVE_NODES
					</p>
					<p class="mt-2 text-sm text-muted-foreground">
						Server is online. Use the setup actions above to deploy your first workload.
					</p>
					<Button class="mt-4" onclick={() => goto('/projects')}>
						<Plus class="mr-1 h-4 w-4" />
						Create Project
					</Button>
				</div>
			{:else}
				<!-- Project Sectors -->
				<div class="grid gap-6">
					{#each projects as project (project.id)}
						<div class="relative">
							<!-- Project label -->
							<div class="mb-3 flex items-center gap-2">
								<div class="font-mono text-xs tracking-wider text-muted-foreground">
									{project.id.slice(0, 4).toUpperCase()}
								</div>
								<button
									onclick={() => goto(`/projects/${project.id}`)}
									class="font-mono text-sm font-semibold tracking-wider text-foreground hover:text-foreground"
								>
									{project.name.toUpperCase()}
								</button>
							</div>

							<!-- Nodes in this project -->
							<div class="flex flex-wrap items-start gap-3">
								<!-- Apps -->
								{#each appsByProject[project.id] ?? [] as app (app.id)}
									<button
										onclick={() => goto(`/apps/${app.id}`)}
										class={cn(
											'flex items-center gap-2 rounded border bg-background px-3 py-2 hover:cursor-pointer',
											appStatusColor(app)
										)}
									>
										<Rocket class="h-4 w-4 shrink-0" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{app.name}
											</div>
											<div
												class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
											>
												{app.status}
											</div>
										</div>
									</button>
								{/each}

								<!-- Databases -->
								{#each dbsByProject[project.id] ?? [] as db (db.id)}
									<button
										onclick={() => goto('/databases')}
										class={cn(
											'flex items-center gap-2 rounded border bg-background px-3 py-2 hover:cursor-pointer',
											dbStatusColor(db)
										)}
									>
										<Database class="h-4 w-4 shrink-0 text-accent" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{db.name}
											</div>
											<div
												class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
											>
												{db.db_type}
											</div>
										</div>
									</button>
								{/each}

								<!-- Cron Jobs -->
								{#each jobsByProject[project.id] ?? [] as job (job.id)}
									<button
										onclick={() => goto('/cron')}
										class={cn(
											'flex items-center gap-2 rounded border bg-background px-3 py-2 hover:cursor-pointer',
											jobStatusColor(job)
										)}
									>
										<Clock class="h-4 w-4 shrink-0 text-secondary" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{job.name}
											</div>
											<div class="font-mono text-[10px] tracking-wider text-muted-foreground">
												{job.schedule}
											</div>
										</div>
									</button>
								{/each}
							</div>
						</div>
					{/each}

					<!-- Unassigned Resources -->
					{#if appsByProject['unassigned']?.length || dbsByProject['unassigned']?.length || jobsByProject['unassigned']?.length}
						<div class="relative border-t border-border pt-4">
							<div class="mb-3 flex items-center gap-2">
								<div class="font-mono text-sm font-semibold tracking-wider text-muted-foreground">
									UNASSIGNED_RESOURCES
								</div>
							</div>

							<div class="flex flex-wrap items-start gap-3">
								{#each appsByProject['unassigned'] ?? [] as app (app.id)}
									<button
										onclick={() => goto(`/apps/${app.id}`)}
										class={cn(
											'flex items-center gap-2 rounded border px-3 py-2 hover:cursor-pointer',
											appStatusColor(app)
										)}
									>
										<Rocket class="h-4 w-4 shrink-0" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{app.name}
											</div>
											<div
												class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
											>
												{app.status}
											</div>
										</div>
									</button>
								{/each}

								{#each dbsByProject['unassigned'] ?? [] as db (db.id)}
									<button
										onclick={() => goto('/databases')}
										class={cn(
											'flex items-center gap-2 rounded border px-3 py-2 hover:cursor-pointer',
											dbStatusColor(db)
										)}
									>
										<Database class="h-4 w-4 shrink-0 text-accent" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{db.name}
											</div>
											<div
												class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase"
											>
												{db.db_type}
											</div>
										</div>
									</button>
								{/each}

								{#each jobsByProject['unassigned'] ?? [] as job (job.id)}
									<button
										onclick={() => goto('/cron')}
										class={cn(
											'flex items-center gap-2 rounded border px-3 py-2 hover:cursor-pointer',
											jobStatusColor(job)
										)}
									>
										<Clock class="h-4 w-4 shrink-0 text-secondary" />
										<div class="text-left">
											<div class="font-mono text-xs tracking-wider text-foreground">
												{job.name}
											</div>
											<div class="font-mono text-[10px] tracking-wider text-muted-foreground">
												{job.schedule}
											</div>
										</div>
									</button>
								{/each}
							</div>
						</div>
					{/if}
				</div>
			{/if}
		</div>
	</div>

	<!-- Quick Access Panel -->
	<div class="grid gap-4 sm:grid-cols-3">
		<!-- Activity Feed -->
		<div class="rounded-lg border border-border bg-card p-4 sm:col-span-2">
			<div class="mb-3 flex items-center justify-between">
				<div class="flex items-center gap-2">
					<Activity class="h-4 w-4 text-muted-foreground" />
					<span class="font-mono text-xs tracking-wider text-muted-foreground uppercase"
						>ACTIVITY_LOG</span
					>
				</div>
				<button
					onclick={() => goto('/activity')}
					class="font-mono text-[10px] tracking-wider text-muted-foreground hover:text-muted-foreground"
				>
					VIEW_ALL →
				</button>
			</div>

			{#if recentActivity.length === 0}
				<p class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
					NO_RECENT_ACTIVITY
				</p>
			{:else}
				<div class="space-y-2">
					{#each recentActivity.slice(0, 5) as entry (entry.id)}
						<div class="flex items-center gap-3 text-xs">
							<span class="font-mono text-muted-foreground">
								{new Date(entry.created_at).toLocaleTimeString()}
							</span>
							<Badge variant={actionVariant(entry.action)} class="font-mono text-[10px]">
								{entry.action.toUpperCase()}
							</Badge>
							<span class="text-muted-foreground">
								{entry.app_name ?? entry.detail ?? 'System'}
							</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- System Status -->
		<div class="rounded-lg border border-border bg-card p-4">
			<div class="mb-3 flex items-center gap-2">
				<Zap class="h-4 w-4 text-muted-foreground" />
				<span class="font-mono text-xs tracking-wider text-muted-foreground uppercase">STATUS</span>
			</div>

			<div class="space-y-3">
				<div class="flex items-center justify-between">
					<span class="font-mono text-[10px] text-muted-foreground uppercase">Apps</span>
					<span class="font-mono text-sm text-foreground">
						{runningApps}/{apps.length}
					</span>
				</div>
				<div class="h-1 overflow-hidden rounded-full bg-muted">
					<div
						class="h-full bg-success/60 transition-all"
						style="width: {apps.length > 0 ? (runningApps / apps.length) * 100 : 0}%"
					></div>
				</div>

				<div class="flex items-center justify-between">
					<span class="font-mono text-[10px] text-muted-foreground uppercase">Databases</span>
					<span class="font-mono text-sm text-foreground">
						{runningDatabases}/{databases.length}
					</span>
				</div>
				<div class="h-1 overflow-hidden rounded-full bg-muted">
					<div
						class="h-full bg-accent/60 transition-all"
						style="width: {databases.length > 0 ? (runningDatabases / databases.length) * 100 : 0}%"
					></div>
				</div>

				<div class="flex items-center justify-between">
					<span class="font-mono text-[10px] text-muted-foreground uppercase">Cron Jobs</span>
					<span class="font-mono text-sm text-foreground">
						{enabledCronJobs}/{cronJobs.length}
					</span>
				</div>
				<div class="h-1 overflow-hidden rounded-full bg-muted">
					<div
						class="h-full bg-secondary/60 transition-all"
						style="width: {cronJobs.length > 0 ? (enabledCronJobs / cronJobs.length) * 100 : 0}%"
					></div>
				</div>
			</div>
		</div>
	</div>
</div>
