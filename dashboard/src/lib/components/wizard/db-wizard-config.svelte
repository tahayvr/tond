<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { dbLogoUrl } from '$lib/components/brand-logos/db-logos.svelte';
	import DatabaseIcon from '~icons/ri/database-2-line';
	import type { DbCatalogEntry, Project } from '$lib/api';
	import { BACKUP_PRESETS, catalogFor } from '$lib/database-ops';

	export type DbType = 'postgres' | 'mysql' | 'mariadb' | 'redis' | 'keydb' | 'mongodb';

	interface Props {
		name: string;
		dbType: DbType;
		selectedProjectId: string;
		projects: Project[];
		/** Major version ('' = the engine's default) */
		version?: string;
		/** Image variant ('' = the engine's own image) */
		variant?: string;
		/** Cron expression of the database's own backups ('' = off) */
		backupSchedule?: string;
		/** What the server can run; empty hides the version and image choices */
		catalog?: DbCatalogEntry[];
	}

	let {
		name = $bindable(''),
		dbType = $bindable('postgres'),
		selectedProjectId = $bindable(''),
		projects,
		version = $bindable(''),
		variant = $bindable(''),
		backupSchedule = $bindable('0 3 * * *'),
		catalog = []
	}: Props = $props();

	let entries = $derived(catalogFor(catalog, dbType));
	let entry = $derived(entries.find((e) => e.variant === variant) ?? entries[0]);

	// Keep the variant and version valid for the chosen engine.
	$effect(() => {
		if (!entries.length) return;
		if (!entries.some((e) => e.variant === variant && e.available)) variant = '';
	});
	$effect(() => {
		if (entry && !entry.versions.includes(version)) version = entry.default_version;
	});

	const backupLabel = $derived(
		backupSchedule
			? (BACKUP_PRESETS.find((p) => p.value === backupSchedule)?.label ?? backupSchedule)
			: 'Off'
	);

	const DB_OPTIONS: { value: DbType; label: string; description: string }[] = [
		{
			value: 'postgres',
			label: 'PostgreSQL',
			description: 'Advanced open-source relational database'
		},
		{
			value: 'mysql',
			label: 'MySQL',
			description: 'Popular open-source relational database'
		},
		{
			value: 'mariadb',
			label: 'MariaDB',
			description: 'MySQL-compatible community-developed database'
		},
		{
			value: 'redis',
			label: 'Redis',
			description: 'In-memory data structure store and cache'
		},
		{
			value: 'keydb',
			label: 'KeyDB',
			description: 'Multithreaded Redis-compatible in-memory store'
		},
		{
			value: 'mongodb',
			label: 'MongoDB',
			description: 'Flexible document-oriented NoSQL database'
		}
	];

	// Per-engine image load error tracking
	let imgErrors = $state<Partial<Record<DbType, boolean>>>({});

	function handleNameInput(e: Event) {
		const raw = (e.target as HTMLInputElement).value;
		name = raw
			.toLowerCase()
			.replace(/\s+/g, '-')
			.replace(/[^a-z0-9-]/g, '')
			.replace(/-{2,}/g, '-');
	}
</script>

<div class="space-y-6">
	<!-- Database Engine -->
	<div class="space-y-3">
		<Label>Database Engine</Label>
		<div class="grid gap-3 sm:grid-cols-3">
			{#each DB_OPTIONS as opt (opt.value)}
				{@const logoUrl = dbLogoUrl(opt.value)}
				{@const hasImgError = imgErrors[opt.value] ?? false}
				<button
					type="button"
					onclick={() => (dbType = opt.value)}
					class="flex flex-col items-start gap-2 rounded border-2 p-4 text-left transition-all {dbType ===
					opt.value
						? 'border-primary bg-primary/5'
						: 'border-border bg-transparent hover:border-border/80 hover:bg-muted/30'}"
				>
					<div class="flex w-full items-center justify-between">
						{#if logoUrl && !hasImgError}
							<img
								src={logoUrl}
								alt={opt.label}
								class="h-6 w-6 object-contain {dbType === opt.value ? '' : 'opacity-50'}"
								onerror={() => (imgErrors[opt.value] = true)}
							/>
						{:else}
							<DatabaseIcon
								class="h-6 w-6 {dbType === opt.value ? 'text-primary' : 'text-muted-foreground'}"
							/>
						{/if}
						{#if dbType === opt.value}
							<div class="h-2 w-2 rounded-full bg-primary"></div>
						{/if}
					</div>
					<div>
						<div
							class="font-mono text-sm font-medium {dbType === opt.value
								? 'text-foreground'
								: 'text-muted-foreground'}"
						>
							{opt.label}
						</div>
						<div class="mt-0.5 text-xs text-muted-foreground">
							{opt.description}
						</div>
					</div>
				</button>
			{/each}
		</div>
	</div>

	{#if entry}
		{#if entries.length > 1}
			<fieldset class="space-y-3">
				<legend class="text-sm font-medium">Image</legend>
				<div class="grid gap-3 sm:grid-cols-3">
					{#each entries as e (e.variant)}
						<label
							class="flex cursor-pointer flex-col gap-1 rounded border-2 p-3 text-left transition-all has-[:disabled]:cursor-not-allowed has-[:disabled]:opacity-60 {variant ===
							e.variant
								? 'border-primary bg-primary/5'
								: 'border-border hover:bg-muted/30'}"
						>
							<span class="flex items-center gap-2 text-sm font-medium">
								<input
									type="radio"
									name="db-variant"
									value={e.variant}
									bind:group={variant}
									disabled={!e.available}
									class="accent-primary"
								/>
								{e.label || 'Standard'}
							</span>
							<span class="text-xs text-muted-foreground">
								{#if !e.available}
									Not available on this server (x86_64 only).
								{:else if e.variant}
									{e.description}
								{:else}
									The official image with the standard extensions.
								{/if}
							</span>
						</label>
					{/each}
				</div>
			</fieldset>
		{/if}
	{/if}

	<!-- Database Name + Project -->
	<div class="grid gap-4 sm:grid-cols-2">
		<div class="space-y-2">
			<Label for="db-name">Database Name *</Label>
			<Input
				id="db-name"
				type="text"
				value={name}
				oninput={handleNameInput}
				placeholder="my-database"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">Lowercase letters, numbers, and hyphens only</p>
		</div>
		<div class="space-y-2">
			<Label>Project</Label>
			<Select.Root type="single" bind:value={selectedProjectId}>
				<Select.Trigger>
					{selectedProjectId
						? (projects.find((p) => p.id === selectedProjectId)?.name ?? 'Select project')
						: 'No project'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">No project</Select.Item>
					{#each projects as project (project.id)}
						<Select.Item value={project.id}>{project.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
	</div>
	{#if entry}
		<div class="grid gap-4 sm:grid-cols-2">
			<div class="space-y-2">
				<Label for="db-version">Version</Label>
				<Select.Root type="single" bind:value={version}>
					<Select.Trigger id="db-version" aria-label="Version">
						{version}{version === entry.default_version ? ' (recommended)' : ''}
					</Select.Trigger>
					<Select.Content>
						{#each [...entry.versions].reverse() as v (v)}
							<Select.Item value={v}>
								{v}{v === entry.default_version ? ' (recommended)' : ''}
							</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				{#if entry.supports_upgrade}
					<p class="text-xs text-muted-foreground">
						You can upgrade to a newer version later; downgrades aren't possible.
					</p>
				{/if}
			</div>
			<div class="space-y-2">
				<Label for="db-backups">Automatic backups</Label>
				<Select.Root type="single" bind:value={backupSchedule}>
					<Select.Trigger id="db-backups" aria-label="Automatic backups"
						>{backupLabel}</Select.Trigger
					>
					<Select.Content>
						{#each BACKUP_PRESETS as p (p.value)}
							<Select.Item value={p.value}>{p.label}</Select.Item>
						{/each}
						<Select.Item value="">Off</Select.Item>
					</Select.Content>
				</Select.Root>
				<p class="text-xs {backupSchedule ? 'text-muted-foreground' : 'text-warn'}">
					{backupSchedule
						? 'Keeps the last 7 backups (UTC schedule). Change it later on the Backups tab.'
						: 'Without a schedule, only full-server snapshots (if turned on) back this database up.'}
				</p>
			</div>
		</div>
	{/if}
</div>
