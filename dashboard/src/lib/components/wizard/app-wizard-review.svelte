<script lang="ts">
	import type { Project, RegistryCredential } from '$lib/api';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';

	interface Volume {
		container_path: string;
		name?: string;
	}

	interface Props {
		name: string;
		selectedProjectId: string;
		sourceSubType: SourceSubType;
		sourceUrl: string;
		githubAppId: string | null;
		dockerfileContent: string;
		composeContent: string;
		buildMethod: 'dockerfile' | 'tbuild';
		containerPort: number;
		autoDeploy: boolean;
		branch: string;
		baseDir: string;
		staticBuild: boolean;
		domain: string;
		/** Settings' apps domain; null while unknown, '' when none is set */
		appsDomain?: string | null;
		healthCheckPath: string;
		healthCheckInterval: number;
		healthFailureThreshold: number;
		drainTimeout: number;
		memoryLimit: number | null;
		memoryUnit: 'mb' | 'gb';
		cpuLimit: number | null;
		volumes: Volume[];
		envVars: Record<string, string>;
		registryCredentialId: string;
		projects: Project[];
		registryCredentials: RegistryCredential[];
	}

	let {
		name,
		selectedProjectId,
		sourceSubType,
		sourceUrl,
		githubAppId,
		dockerfileContent,
		composeContent,
		buildMethod,
		containerPort,
		autoDeploy,
		branch,
		baseDir,
		staticBuild,
		domain,
		appsDomain = null,
		healthCheckPath,
		healthCheckInterval,
		healthFailureThreshold,
		drainTimeout,
		memoryLimit,
		memoryUnit,
		cpuLimit,
		volumes,
		envVars,
		registryCredentialId,
		projects,
		registryCredentials
	}: Props = $props();

	const SOURCE_LABELS: Record<SourceSubType, string> = {
		'git-public': 'Git — Public Repo',
		'git-github-app': 'Git — Private Repo (GitHub App)',
		dockerfile: 'Docker — Dockerfile',
		compose: 'Docker — Docker Compose',
		image: 'Docker — Image',
		template: 'Template'
	};

	const BUILD_METHOD_LABELS: Record<string, string> = {
		tbuild: 'TBuild',
		dockerfile: 'Dockerfile'
	};

	let isGit = $derived(
		sourceSubType === 'git-public' || sourceSubType === 'git-github-app' || !!branch
	);
	let envKeys = $derived(Object.keys(envVars));

	function getMemoryDisplay(limit: number | null, unit: 'mb' | 'gb'): string {
		if (!limit) return 'No app limit (host cap: 75% of RAM)';
		return `${limit} ${unit.toUpperCase()}`;
	}

	function getCpuDisplay(limit: number | null): string {
		if (!limit) return 'All cores';
		return `${limit} CPU${limit !== 1 ? 's' : ''}`;
	}
</script>

<div class="space-y-4">
	<div class="grid gap-4 sm:grid-cols-2">
		<!-- Source -->
		<div class="space-y-3 rounded border border-dotted p-4">
			<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Source
			</h4>
			<div class="space-y-1 text-sm">
				<div><span class="text-muted-foreground">Name:</span> {name}</div>
				<div><span class="text-muted-foreground">Type:</span> {SOURCE_LABELS[sourceSubType]}</div>
				{#if sourceSubType === 'git-public'}
					<div class="truncate">
						<span class="text-muted-foreground">URL:</span>
						<span class="font-mono text-xs">{sourceUrl}</span>
					</div>
				{:else if sourceSubType === 'git-github-app'}
					<div>
						<span class="text-muted-foreground">Repo:</span>
						<span class="font-mono text-xs">{githubAppId ?? '—'}</span>
					</div>
				{:else if sourceSubType === 'image'}
					<div class="truncate">
						<span class="text-muted-foreground">Image:</span>
						<span class="font-mono text-xs">{sourceUrl}</span>
					</div>
				{:else if sourceSubType === 'template'}
					<div class="truncate">
						<span class="text-muted-foreground">Template Source:</span>
						<span class="font-mono text-xs">{sourceUrl}</span>
					</div>
				{:else if sourceSubType === 'dockerfile'}
					<div class="text-muted-foreground">
						Dockerfile inline ({dockerfileContent.trim().split('\n').length} lines)
					</div>
				{:else if sourceSubType === 'compose'}
					<div class="text-muted-foreground">
						docker-compose.yml inline ({composeContent.trim().split('\n').length} lines)
					</div>
				{/if}
				{#if selectedProjectId}
					<div>
						<span class="text-muted-foreground">Project:</span>
						{projects.find((p) => p.id === selectedProjectId)?.name ?? selectedProjectId}
					</div>
				{:else}
					<div class="text-muted-foreground">No project</div>
				{/if}
			</div>
		</div>

		<!-- Build -->
		<div class="space-y-3 rounded border border-dotted p-4">
			<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Build
			</h4>
			<div class="space-y-1 text-sm">
				{#if isGit}
					<div>
						<span class="text-muted-foreground">Method:</span>
						{BUILD_METHOD_LABELS[buildMethod] ?? buildMethod}
					</div>
				{:else if sourceSubType === 'dockerfile'}
					<div><span class="text-muted-foreground">Method:</span> Inline Dockerfile</div>
				{:else if sourceSubType === 'compose'}
					<div><span class="text-muted-foreground">Method:</span> Docker Compose</div>
				{:else if sourceSubType === 'image'}
					<div><span class="text-muted-foreground">Method:</span> Pre-built image</div>
				{/if}
				<div><span class="text-muted-foreground">Container Port:</span> {containerPort}</div>
				{#if baseDir}
					<div>
						<span class="text-muted-foreground">Base Directory:</span>
						<span class="font-mono text-xs">{baseDir}</span>
					</div>
				{/if}
				<div>
					<span class="text-muted-foreground">Static Build:</span>
					{staticBuild ? 'Yes' : 'No'}
				</div>
				{#if isGit}
					<div>
						<span class="text-muted-foreground">Branch:</span>
						<span class="font-mono text-xs">{branch || 'main'}</span>
					</div>
					<div>
						<span class="text-muted-foreground">Auto Deploy:</span>
						{autoDeploy ? `Yes, on push to ${branch || 'main'}` : 'No'}
					</div>
				{/if}
			</div>
		</div>

		<!-- Network -->
		<div class="space-y-3 rounded border border-dotted p-4">
			<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Network
			</h4>
			<div class="space-y-1 text-sm">
				<div>
					<span class="text-muted-foreground">Domain:</span>
					{#if domain}
						{domain}
					{:else if appsDomain}
						<span class="font-mono">{name}.{appsDomain}</span> (automatic)
					{:else if appsDomain === ''}
						<span class="text-warn">None — add a custom domain after creating the app</span>
					{:else}
						Automatic
					{/if}
				</div>
				<div>
					<span class="text-muted-foreground">Health Path:</span>
					{healthCheckPath || 'Disabled'}
				</div>
				{#if healthCheckPath}
					<div>
						<span class="text-muted-foreground">Check Interval:</span>
						{healthCheckInterval}s
					</div>
					<div>
						<span class="text-muted-foreground">Failure Threshold:</span>
						{healthFailureThreshold}
					</div>
				{/if}
				<div><span class="text-muted-foreground">Drain Timeout:</span> {drainTimeout}s</div>
			</div>
		</div>

		<!-- Resources -->
		<div class="space-y-3 rounded border border-dotted p-4">
			<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Resources
			</h4>
			<div class="space-y-1 text-sm">
				<div>
					<span class="text-muted-foreground">Memory:</span>
					{getMemoryDisplay(memoryLimit, memoryUnit)}
				</div>
				<div>
					<span class="text-muted-foreground">CPU:</span>
					{getCpuDisplay(cpuLimit)}
				</div>
				{#if volumes.length > 0}
					<div class="pt-1">
						<div class="text-muted-foreground">Volumes ({volumes.length}):</div>
						{#each volumes as vol (vol.container_path)}
							<div class="mt-0.5 truncate font-mono text-xs text-muted-foreground">
								↳ {vol.container_path}
							</div>
						{/each}
					</div>
				{:else}
					<div><span class="text-muted-foreground">Volumes:</span> None</div>
				{/if}
			</div>
		</div>
	</div>

	<!-- Environment -->
	<div class="space-y-3 rounded border border-dotted p-4">
		<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
			Environment
		</h4>
		<div class="space-y-1 text-sm">
			{#if envKeys.length > 0}
				<div class="flex flex-wrap gap-1.5">
					{#each envKeys as key (key)}
						<span
							class="rounded border border-border bg-background px-1.5 py-0.5 font-mono text-xs"
						>
							{key}
						</span>
					{/each}
				</div>
			{:else}
				<div class="text-muted-foreground">No environment variables</div>
			{/if}
			{#if registryCredentialId}
				<div class="pt-1">
					<span class="text-muted-foreground">Registry:</span>
					{registryCredentials.find((c) => c.id === registryCredentialId)?.name ??
						registryCredentialId}
				</div>
			{/if}
		</div>
	</div>
</div>
