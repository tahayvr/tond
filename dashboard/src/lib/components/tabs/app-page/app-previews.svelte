<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { safeUrl } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { can } from '$lib/auth.svelte';
	import type { PreviewDeployment } from '$lib/api';
	import PreviewMetricsSheet from './preview-metrics-sheet.svelte';
	import ChartIcon from '~icons/ri/line-chart-line';
	import Github from '~icons/ri/github-fill';
	import GitBranch from '~icons/ri/git-branch-line';
	import ExternalLink from '~icons/ri/external-link-line';

	interface Props {
		previews: PreviewDeployment[];
		loading: boolean;
		error: string;
		onDelete: (id: string) => void;
	}

	let { previews, loading, error, onDelete }: Props = $props();

	/** Preview whose metrics are open in the side panel. */
	let metricsFor = $state<PreviewDeployment | null>(null);

	function shortSha(sha: string): string {
		return sha.slice(0, 7);
	}
</script>

<div class="space-y-6 pt-4">
	<InlineError message={error} />

	<!-- Active previews list -->
	{#if loading}
		<div class="py-8 text-center text-muted-foreground">Loading...</div>
	{:else if previews.length === 0}
		<div class="rounded-lg border border-dashed py-12 text-center">
			<p class="text-muted-foreground">No active preview deployments</p>
			<p class="mt-1 text-sm text-muted-foreground">Create a preview deployment below</p>
		</div>
	{:else}
		<div class="overflow-x-auto rounded-md border">
			<table class="w-full text-sm">
				<thead>
					<tr class="border-b bg-muted/50">
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Status</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">PR</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Branch</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">URL</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Image</th>
						<th class="px-4 py-2 text-left font-medium text-muted-foreground">Expires</th>
						<th class="w-40 px-4 py-2"><span class="sr-only">Actions</span></th>
					</tr>
				</thead>
				<tbody>
					{#each previews as p (p.id)}
						<tr class="border-b last:border-0">
							<td class="px-4 py-2">
								<StatusBadge status={p.status} kind="preview" size="sm" />
							</td>

							<!-- PR column -->
							<td class="px-4 py-2">
								{#if p.pr_number !== null && p.pr_url}
									<a
										href={safeUrl(p.pr_url)}
										target="_blank"
										rel="noopener noreferrer"
										class="inline-flex items-center gap-1 font-mono text-xs text-foreground underline-offset-2 hover:underline"
									>
										<Github class="h-3 w-3 shrink-0" />
										#{p.pr_number}
									</a>
								{:else if p.pr_number !== null}
									<span
										class="inline-flex items-center gap-1 font-mono text-xs text-muted-foreground"
									>
										<Github class="h-3 w-3 shrink-0" />
										#{p.pr_number}
									</span>
								{:else}
									<span class="text-xs text-muted-foreground">—</span>
								{/if}
							</td>

							<!-- Branch / commit column -->
							<td class="px-4 py-2">
								{#if p.head_ref}
									<div class="flex flex-col gap-0.5">
										<span class="inline-flex items-center gap-1 font-mono text-xs text-foreground">
											<GitBranch class="h-3 w-3 shrink-0 text-muted-foreground" />
											{p.head_ref}
										</span>
										{#if p.head_sha}
											<code class="font-mono text-[10px] text-muted-foreground"
												>{shortSha(p.head_sha)}</code
											>
										{/if}
									</div>
								{:else}
									<span class="text-xs text-muted-foreground">—</span>
								{/if}
							</td>

							<!-- Subdomain URL -->
							<td class="px-4 py-2">
								{#if p.subdomain}
									<a
										href={safeUrl(`https://${p.subdomain}`)}
										target="_blank"
										rel="noopener noreferrer"
										class="inline-flex items-center gap-1 font-mono text-xs text-foreground underline-offset-2 hover:underline"
									>
										{p.subdomain}
										<ExternalLink class="h-3 w-3 shrink-0" />
									</a>
								{:else}
									<span class="text-xs text-muted-foreground">—</span>
								{/if}
							</td>

							<td class="px-4 py-2 font-mono text-xs text-muted-foreground">{p.image_tag ?? '—'}</td
							>
							<td class="px-4 py-2 text-xs text-muted-foreground"
								>{new Date(p.expires_at).toLocaleString()}</td
							>
							<td class="px-4 py-2">
								<div class="flex items-center justify-end gap-1">
									{#if can('metrics:read')}
										<Button
											variant="ghost"
											size="sm"
											onclick={() => (metricsFor = p)}
											aria-label="Metrics for preview {p.pr_number !== null
												? `#${p.pr_number}`
												: p.slug}"
										>
											<ChartIcon class="size-4" aria-hidden="true" />
											Metrics
										</Button>
									{/if}
									<Button
										variant="ghost"
										size="sm"
										class="text-destructive hover:text-destructive"
										onclick={() => onDelete(p.id)}
									>
										Delete
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

<PreviewMetricsSheet preview={metricsFor} onclose={() => (metricsFor = null)} />
