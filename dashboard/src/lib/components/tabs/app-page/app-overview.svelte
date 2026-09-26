<script lang="ts">
	import { safeUrl } from '$lib/utils';
	import { can } from '$lib/auth.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import AppOverviewMetrics from './app-overview-metrics.svelte';
	import UptimeBadge from '$lib/components/uptime/UptimeBadge.svelte';
	import type { App } from '$lib/api';
	import LayoutTemplate from '~icons/ri/layout-masonry-line';

	interface Props {
		app: App;
		/** Link to the Uptime tab; the uptime summary shows when set. */
		uptimeHref?: string | null;
	}

	let { app, uptimeHref = null }: Props = $props();

	const SOURCE_LABELS: Record<string, string> = {
		git: 'Git — Public Repo',
		'git-public': 'Git — Public Repo',
		'git-github-app': 'Git — Private Repo (GitHub App)',
		dockerfile: 'Docker — Dockerfile',
		compose: 'Docker — Docker Compose',
		image: 'Docker — Image',
		tarball: 'Tarball Upload'
	};

	const BUILD_METHOD_LABELS: Record<string, string> = {
		tbuild: 'TBuild',
		dockerfile: 'Dockerfile'
	};

	let isGit = $derived(
		app.source_type === 'git' ||
			app.source_type === 'git-public' ||
			app.source_type === 'git-github-app'
	);

	function memoryDisplay(bytes: number | null): string {
		if (!bytes) return 'Unlimited';
		const mib = bytes / 1_048_576;
		if (mib >= 1024) return `${(mib / 1024).toFixed(1)} GiB`;
		return `${Math.round(mib)} MiB`;
	}

	function cpuDisplay(nanoCpu: number | null): string {
		if (!nanoCpu) return 'Unlimited';
		const cores = nanoCpu / 1_000_000_000;
		return `${cores % 1 === 0 ? cores.toFixed(0) : cores.toFixed(2)} CPU${cores !== 1 ? 's' : ''}`;
	}
</script>

<div class="space-y-4 pt-4">
	{#if app.template_id}
		<p class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
			<LayoutTemplate class="size-4 shrink-0" aria-hidden="true" />
			<span>
				Deployed from the
				<a
					href="/templates/{app.template_id}"
					class="text-foreground underline-offset-4 hover:underline">{app.template_id}</a
				>
				template{#if app.template_version}&nbsp;({app.template_version}){/if}
			</span>
			<span aria-hidden="true">·</span>
			<a
				href="/templates/{app.template_id}#upgrading"
				class="text-foreground underline-offset-4 hover:underline">Upgrade notes</a
			>
		</p>
	{/if}
	{#if uptimeHref && app.domain}
		<UptimeBadge appId={app.id} href={uptimeHref} />
	{/if}
	{#if can('metrics:read')}
		<AppOverviewMetrics appId={app.id} />
	{/if}
	<div class="grid grid-cols-1 gap-4 sm:grid-cols-2 [&>*]:min-w-0">
		<!-- Status -->
		<div class="min-w-0 space-y-3 rounded border border-dotted p-4">
			<h2 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Status
			</h2>
			<div class="space-y-1 text-sm">
				<div class="flex items-center gap-2">
					<span class="text-muted-foreground">State:</span>
					<StatusBadge status={app.status} kind="app" />
					{#if app.health_consecutive_failures > 0}
						<StatusBadge
							status="failed"
							label={`${app.health_consecutive_failures} failed health check${app.health_consecutive_failures !== 1 ? 's' : ''}`}
						/>
					{/if}
				</div>
				<div>
					<span class="text-muted-foreground">App ID:</span>
					<span class="font-mono text-xs break-all">{app.id}</span>
				</div>
				<div>
					<span class="text-muted-foreground">Created:</span>
					{new Date(app.created_at).toLocaleString()}
				</div>
				<div>
					<span class="text-muted-foreground">Updated:</span>
					{new Date(app.updated_at).toLocaleString()}
				</div>
			</div>
		</div>

		<!-- Source -->
		<div class="min-w-0 space-y-3 rounded border border-dotted p-4">
			<h2 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Source
			</h2>
			<div class="space-y-1 text-sm">
				<div>
					<span class="text-muted-foreground">Type:</span>
					{SOURCE_LABELS[app.source_type] ?? app.source_type}
				</div>
				{#if app.source_url && app.source_type !== 'dockerfile' && app.source_type !== 'compose'}
					<div class="truncate">
						<span class="text-muted-foreground"
							>{app.source_type === 'image' ? 'Image' : 'URL'}:</span
						>
						<span class="font-mono text-xs break-all">{app.source_url}</span>
					</div>
				{:else if app.source_type === 'dockerfile' && app.dockerfile_content}
					<div class="text-muted-foreground">
						Dockerfile inline ({app.dockerfile_content.trim().split('\n').length} lines)
					</div>
				{:else if app.source_type === 'compose' && app.compose_content}
					<div class="text-muted-foreground">
						docker-compose.yml inline ({app.compose_content.trim().split('\n').length} lines)
					</div>
				{/if}
				{#if isGit && app.auto_deploy}
					<div>
						<span class="text-muted-foreground">Auto Deploy:</span> Yes
					</div>
					{#if app.watch_branch}
						<div>
							<span class="text-muted-foreground">Watch Branch:</span>
							<span class="font-mono text-xs break-all">{app.watch_branch}</span>
						</div>
					{/if}
				{:else if isGit}
					<div>
						<span class="text-muted-foreground">Auto Deploy:</span> No
					</div>
				{/if}
			</div>
		</div>

		<!-- Build -->
		<div class="min-w-0 space-y-3 rounded border border-dotted p-4">
			<h2 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Build
			</h2>
			<div class="space-y-1 text-sm">
				{#if isGit}
					<div>
						<span class="text-muted-foreground">Method:</span>
						{app.build_method
							? (BUILD_METHOD_LABELS[app.build_method] ?? app.build_method)
							: 'TBuild'}
					</div>
				{:else if app.source_type === 'dockerfile'}
					<div><span class="text-muted-foreground">Method:</span> Inline Dockerfile</div>
				{:else if app.source_type === 'compose'}
					<div><span class="text-muted-foreground">Method:</span> Docker Compose</div>
				{:else if app.source_type === 'image'}
					<div><span class="text-muted-foreground">Method:</span> Pre-built image</div>
				{/if}
				{#if app.base_dir}
					<div>
						<span class="text-muted-foreground">Base Directory:</span>
						<span class="font-mono text-xs break-all">{app.base_dir}</span>
					</div>
				{/if}
				<div>
					<span class="text-muted-foreground">Static Build:</span>
					{app.static_build ? 'Yes' : 'No'}
				</div>
				<div>
					<span class="text-muted-foreground">Container Port:</span>
					{app.container_port}
				</div>
				{#if app.host_port}
					<div>
						<span class="text-muted-foreground">Host Port:</span>
						{app.host_port}
					</div>
				{/if}
			</div>
		</div>

		<!-- Network -->
		<div class="min-w-0 space-y-3 rounded border border-dotted p-4">
			<h2 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Network
			</h2>
			<div class="space-y-1 text-sm">
				<div>
					<span class="text-muted-foreground">Domain:</span>
					{#if app.domain}
						<a
							href={safeUrl(`https://${app.domain}`)}
							target="_blank"
							rel="noopener noreferrer"
							class="font-mono text-xs text-muted-foreground underline hover:text-foreground"
							>{app.domain}</a
						>
					{:else}
						<span class="text-muted-foreground">Not set</span>
					{/if}
				</div>
				<div>
					<span class="text-muted-foreground">Health Path:</span>
					{#if app.health_check_path}
						<span class="font-mono text-xs break-all">{app.health_check_path}</span>
					{:else}
						Disabled
					{/if}
				</div>
				{#if app.health_check_path}
					<div>
						<span class="text-muted-foreground">Check Interval:</span>
						{app.health_check_interval}s
					</div>
					<div>
						<span class="text-muted-foreground">Failure Threshold:</span>
						{app.health_failure_threshold}
					</div>
				{/if}
				<div>
					<span class="text-muted-foreground">Drain Timeout:</span>
					{app.drain_timeout}s
				</div>
			</div>
		</div>
	</div>

	<!-- Resources (full width) -->
	<div class="min-w-0 space-y-3 rounded border border-dotted p-4">
		<h2 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
			Resources
		</h2>
		<div class="grid gap-x-8 gap-y-1 text-sm sm:grid-cols-2">
			<div>
				<span class="text-muted-foreground">Memory:</span>
				{memoryDisplay(app.memory_limit)}
			</div>
			<div>
				<span class="text-muted-foreground">CPU:</span>
				{cpuDisplay(app.cpu_limit)}
			</div>
			{#if app.volumes.length > 0}
				<div class="min-w-0 pt-1 sm:col-span-2">
					<div class="text-muted-foreground">Volumes ({app.volumes.length}):</div>
					{#each app.volumes as vol (vol.container_path)}
						<div class="mt-0.5 font-mono text-xs break-all text-muted-foreground">
							↳ {vol.host_path} → {vol.container_path}
						</div>
					{/each}
				</div>
			{:else}
				<div><span class="text-muted-foreground">Volumes:</span> None</div>
			{/if}
		</div>
	</div>
</div>
