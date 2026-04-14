<script lang="ts">
	import type { Project } from '$lib/api';
	import type { DbType } from '$lib/components/wizard/db-wizard-config.svelte';

	interface Props {
		name: string;
		dbType: DbType;
		selectedProjectId: string;
		projects: Project[];
	}

	let { name, dbType, selectedProjectId, projects }: Props = $props();

	const DB_TYPE_LABELS: Record<DbType, string> = {
		postgres: 'PostgreSQL',
		mysql: 'MySQL',
		mariadb: 'MariaDB',
		redis: 'Redis',
		keydb: 'KeyDB',
		mongodb: 'MongoDB'
	};

	// Engines that do not use credentials (no db_name / db_user / db_password)
	const NO_CREDENTIALS_ENGINES: DbType[] = ['redis', 'keydb'];

	let usesCredentials = $derived(!NO_CREDENTIALS_ENGINES.includes(dbType));
</script>

<div class="space-y-4">
	<div class="space-y-3 rounded border border-dotted p-4">
		<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
			Configuration
		</h4>
		<div class="space-y-1 text-sm">
			<div><span class="text-muted-foreground">Name:</span> {name}</div>
			<div>
				<span class="text-muted-foreground">Engine:</span>
				{DB_TYPE_LABELS[dbType]}
			</div>
			{#if selectedProjectId}
				<div>
					<span class="text-muted-foreground">Project:</span>
					{projects.find((p) => p.id === selectedProjectId)?.name ?? selectedProjectId}
				</div>
			{:else}
				<div class="text-muted-foreground">No project assigned</div>
			{/if}
		</div>
	</div>

	<!-- Credentials note -->
	<div class="space-y-3 rounded border border-dotted p-4">
		<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
			Credentials
		</h4>
		<div class="space-y-1 text-sm text-muted-foreground">
			{#if usesCredentials}
				<p>
					A database named <span class="font-mono text-foreground"
						>{name.replace(/-/g, '_') || '…'}</span
					> will be created with auto-generated credentials. Connection strings will be available on the
					database detail page after provisioning.
				</p>
			{:else}
				<p>
					{DB_TYPE_LABELS[dbType]} does not use credentials. A connection string will be available on
					the detail page after provisioning.
				</p>
			{/if}
		</div>
	</div>
</div>
