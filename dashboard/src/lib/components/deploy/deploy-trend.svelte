<!--
  DeployTrend — success rate and median duration over the last 30 deploys,
  with one cell per deploy (oldest left). Loads its own data.

  <DeployTrend appId={app.id} refreshKey={deployments.length} />
-->
<script lang="ts">
	import { getDeployStats, type DeployOutcome, type DeployStats } from '$lib/api';
	import { formatDuration, shortSha } from '$lib/deploy-view';
	import { cn } from '$lib/utils';

	let { appId, refreshKey = 0 }: { appId: string; refreshKey?: unknown } = $props();

	let stats = $state<DeployStats | null>(null);

	$effect(() => {
		const id = appId;
		void refreshKey;
		getDeployStats(id)
			.then((s) => {
				if (id === appId) stats = s;
			})
			.catch(() => {
				// Optional context: the list below still works without it.
			});
	});

	let rate = $derived(stats?.success_rate != null ? Math.round(stats.success_rate * 100) : null);
	let rateTone = $derived(
		rate === null ? '' : rate >= 90 ? 'text-success' : rate >= 70 ? 'text-warn' : 'text-destructive'
	);

	function cellClass(o: DeployOutcome): string {
		if (o.status === 'succeeded') return 'bg-success';
		if (o.status === 'failed') return 'bg-destructive';
		return 'bg-muted-foreground/30';
	}

	function cellLabel(o: DeployOutcome): string {
		const when = new Date(o.started_at).toLocaleString();
		const what = o.commit_sha ? `${shortSha(o.commit_sha)} ` : '';
		const took = o.duration_secs != null ? `, ${formatDuration(o.duration_secs)}` : '';
		const status =
			o.status === 'succeeded' ? 'succeeded' : o.status === 'failed' ? 'failed' : 'replaced';
		return `${what}${status}${took} · ${when}`;
	}
</script>

{#if stats && stats.window > 0}
	<section
		class="grid gap-4 rounded-md border p-3 sm:grid-cols-[auto_auto_minmax(0,1fr)] sm:items-center sm:gap-6"
		aria-label="Deploy trend"
	>
		<div>
			<p class="text-xs text-muted-foreground">Success rate</p>
			<p class={cn('text-2xl font-semibold tabular-nums', rateTone)}>
				{rate === null ? '—' : `${rate}%`}
			</p>
			<p class="text-xs text-muted-foreground">
				{stats.succeeded} of {stats.succeeded + stats.failed}, last {stats.window} deploys
			</p>
		</div>
		<div>
			<p class="text-xs text-muted-foreground">Median duration</p>
			<p class="text-2xl font-semibold tabular-nums">
				{stats.median_duration_secs != null ? formatDuration(stats.median_duration_secs) : '—'}
			</p>
			<p class="text-xs text-muted-foreground">of successful deploys</p>
		</div>
		<div class="min-w-0">
			<ol class="flex h-8 items-end gap-0.5" aria-label="Deploy results, oldest first">
				{#each stats.outcomes as o (o.id)}
					<li class="flex h-full min-w-0 flex-1 items-end">
						<a
							href="/apps/{appId}/deployments/{o.id}"
							title={cellLabel(o)}
							aria-label={cellLabel(o)}
							class={cn(
								'block h-full w-full max-w-3 rounded-sm opacity-90 transition-opacity hover:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/50',
								cellClass(o),
								o.status !== 'succeeded' && o.status !== 'failed' && 'h-1/2 self-end'
							)}
						></a>
					</li>
				{/each}
			</ol>
			<p class="mt-1.5 flex flex-wrap gap-x-3 text-xs text-muted-foreground">
				<span class="inline-flex items-center gap-1"
					><span class="size-2 rounded-sm bg-success" aria-hidden="true"></span>Succeeded</span
				>
				<span class="inline-flex items-center gap-1"
					><span class="size-2 rounded-sm bg-destructive" aria-hidden="true"></span>Failed</span
				>
				<span class="inline-flex items-center gap-1"
					><span class="size-2 rounded-sm bg-muted-foreground/30" aria-hidden="true"
					></span>Replaced</span
				>
			</p>
		</div>
	</section>
{/if}
