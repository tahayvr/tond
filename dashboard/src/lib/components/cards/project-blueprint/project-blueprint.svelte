<script lang="ts">
	import ZapIcon from '~icons/ri/flashlight-line';
	import DatabaseIcon from '~icons/ri/database-2-line';
	import { cn } from '$lib/utils';
	import type { App, ManagedDatabase } from '$lib/api';

	interface Props {
		projectName: string;
		apps: App[];
		databases: ManagedDatabase[];
	}

	let { projectName, apps, databases }: Props = $props();

	function appStatusColor(app: App): string {
		switch (app.status) {
			case 'running':
				return 'border-white bg-white';
			case 'failed':
				return 'border-red-500/60 bg-red-500/10';
			case 'unhealthy':
				return 'border-yellow-500/60 bg-yellow-500/10';
			default:
				return 'border-white/20 bg-white/5';
		}
	}

	function appIconColor(app: App): string {
		switch (app.status) {
			case 'running':
				return 'text-black';
			case 'failed':
				return 'text-red-400';
			case 'unhealthy':
				return 'text-yellow-400';
			default:
				return 'text-white/50';
		}
	}

	function appLabelColor(app: App): string {
		switch (app.status) {
			case 'running':
				return 'text-white/90';
			case 'failed':
				return 'text-red-400/80';
			case 'unhealthy':
				return 'text-yellow-400/80';
			default:
				return 'text-white/35';
		}
	}

	function dbTypeLabel(db: ManagedDatabase): string {
		return db.db_type.toUpperCase();
	}

	function dbStatusColor(db: ManagedDatabase): string {
		return db.status === 'running'
			? 'border-white/30 bg-white/5'
			: 'border-white/10 bg-white/[0.02]';
	}

	function dbIconColor(db: ManagedDatabase): string {
		return db.status === 'running' ? 'text-white/60' : 'text-white/25';
	}

	function dbLabelColor(db: ManagedDatabase): string {
		return db.status === 'running' ? 'text-white/50' : 'text-white/25';
	}
</script>

<div class="relative overflow-hidden rounded-lg border border-white/10 bg-[#111111]">
	<!-- dot grid background -->
	<div
		class="pointer-events-none absolute inset-0"
		style="background-image: radial-gradient(circle, rgba(255,255,255,0.06) 1px, transparent 1px); background-size: 20px 20px;"
	></div>

	<div class="relative flex flex-col items-center gap-8 px-8 py-10">
		<!-- Header label + project name -->
		<div class="flex flex-col items-center gap-1 text-center">
			<span class="font-mono text-[10px] tracking-[0.3em] text-white/30 uppercase">
				TARGET_UNIT_BLUEPRINT
			</span>
			<h2
				class="font-mono text-3xl leading-none font-black tracking-widest text-white/90 uppercase italic"
			>
				{projectName}
			</h2>
		</div>

		<!-- Nodes -->
		{#if apps.length === 0 && databases.length === 0}
			<p class="font-mono text-[11px] tracking-widest text-white/25 uppercase">NO UNITS ASSIGNED</p>
		{:else}
			<div class="flex flex-wrap items-start justify-center gap-8">
				<!-- One node per app -->
				{#each apps as app (app.id)}
					<div class="flex flex-col items-center gap-2">
						<div
							class={cn(
								'flex h-16 w-16 items-center justify-center rounded border transition-colors',
								appStatusColor(app)
							)}
						>
							<ZapIcon class={cn('h-8 w-8 transition-colors', appIconColor(app))} />
						</div>
						<!-- app name -->
						<span
							class="max-w-18 truncate text-center font-mono text-[10px] tracking-wider text-white/50 uppercase"
						>
							{app.name}
						</span>
						<!-- status -->
						<span
							class={cn(
								'-mt-1 font-mono text-[11px] tracking-widest uppercase',
								appLabelColor(app)
							)}
						>
							{app.status.toUpperCase()}
						</span>
					</div>
				{/each}

				<!-- Divider between apps and dbs, only when both exist -->
				{#if apps.length > 0 && databases.length > 0}
					<div class="h-12 w-px self-center bg-white/10"></div>
				{/if}

				<!-- One node per database -->
				{#each databases as db (db.id)}
					<div class="flex flex-col items-center gap-2">
						<div
							class={cn(
								'flex h-16 w-16 items-center justify-center rounded border transition-colors',
								dbStatusColor(db)
							)}
						>
							<DatabaseIcon class={cn('h-8 w-8 transition-colors', dbIconColor(db))} />
						</div>
						<!-- db name -->
						<span
							class="max-w-18 truncate text-center font-mono text-[10px] tracking-wider text-white/50 uppercase"
						>
							{db.name}
						</span>
						<!-- db type -->
						<span
							class={cn('-mt-1 font-mono text-[11px] tracking-widest uppercase', dbLabelColor(db))}
						>
							{dbTypeLabel(db)}
						</span>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
