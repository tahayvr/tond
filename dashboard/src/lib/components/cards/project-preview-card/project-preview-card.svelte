<script lang="ts">
	import { safeUrl } from '$lib/utils';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import type { App, ProjectPreview } from '$lib/api';
	import ExternalLink from '~icons/ri/external-link-line';
	import Trash2 from '~icons/ri/delete-bin-line';
	import ChevronDown from '~icons/ri/arrow-down-s-line';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import Github from '~icons/ri/github-fill';
	import GitBranch from '~icons/ri/git-branch-line';

	let {
		preview,
		apps = [],
		onDelete,
		deleting = false
	}: {
		preview: ProjectPreview;
		apps?: App[];
		onDelete: (id: string) => void;
		deleting?: boolean;
	} = $props();

	let expanded = $state(false);

	function statusVariant(status: string) {
		if (status === 'running') return 'default';
		if (status === 'failed' || status === 'partially_failed') return 'destructive';
		return 'secondary';
	}

	function formatExpiry(dt: string) {
		return new Date(dt).toLocaleString();
	}

	function shortSha(sha: string): string {
		return sha.slice(0, 7);
	}

	function appName(appId: string): string {
		return apps.find((a) => a.id === appId)?.name ?? appId.slice(0, 12);
	}
</script>

<Card.Root class="rounded-sm">
	<Card.Header class="pb-3">
		<div class="flex items-center justify-between gap-2">
			<div class="flex items-center gap-3">
				<button
					type="button"
					onclick={() => (expanded = !expanded)}
					class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground"
					aria-label={expanded ? 'Collapse' : 'Expand'}
				>
					{#if expanded}
						<ChevronDown class="h-4 w-4" />
					{:else}
						<ChevronRight class="h-4 w-4" />
					{/if}
				</button>
				<div>
					<p class="font-mono text-sm font-medium">{preview.slug}</p>
					<p class="text-xs text-muted-foreground">Expires {formatExpiry(preview.expires_at)}</p>
				</div>
			</div>
			<div class="flex items-center gap-2">
				<Badge variant={statusVariant(preview.status)}>{preview.status}</Badge>
				<Button
					variant="ghost"
					size="sm"
					class="text-destructive hover:text-destructive"
					disabled={deleting}
					onclick={() => onDelete(preview.id)}
				>
					<Trash2 class="h-4 w-4" />
				</Button>
			</div>
		</div>
	</Card.Header>

	{#if expanded}
		<Card.Content class="pt-0">
			{#if preview.previews.length === 0}
				<p class="text-sm text-muted-foreground">No app previews yet.</p>
			{:else}
				<div class="rounded-md border">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b bg-muted/50">
								<th class="px-3 py-2 text-left font-medium text-muted-foreground">App</th>
								<th class="px-3 py-2 text-left font-medium text-muted-foreground">Status</th>
								<th class="px-3 py-2 text-left font-medium text-muted-foreground">PR</th>
								<th class="px-3 py-2 text-left font-medium text-muted-foreground">Branch</th>
								<th class="px-3 py-2 text-left font-medium text-muted-foreground">URL</th>
							</tr>
						</thead>
						<tbody>
							{#each preview.previews as p (p.id)}
								<tr class="border-b last:border-0">
									<td class="px-3 py-2 font-mono text-xs">{appName(p.app_id)}</td>
									<td class="px-3 py-2">
										<Badge variant={statusVariant(p.status)}>{p.status}</Badge>
									</td>

									<!-- PR column -->
									<td class="px-3 py-2">
										{#if p.pr_number !== null && p.pr_url}
											<a
												href={safeUrl(p.pr_url)}
												target="_blank"
												rel="noopener noreferrer"
												class="inline-flex items-center gap-1 font-mono text-xs text-accent hover:underline"
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
									<td class="px-3 py-2">
										{#if p.head_ref}
											<div class="flex flex-col gap-0.5">
												<span
													class="inline-flex items-center gap-1 font-mono text-xs text-foreground"
												>
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

									<!-- URL column -->
									<td class="px-3 py-2">
										{#if p.url}
											<a
												href={safeUrl(p.url)}
												target="_blank"
												rel="noopener noreferrer"
												class="inline-flex items-center gap-1 font-mono text-xs text-accent hover:underline"
											>
												{p.subdomain ?? p.url}
												<ExternalLink class="h-3 w-3" />
											</a>
										{:else}
											<span class="text-xs text-muted-foreground">—</span>
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</Card.Content>
	{/if}
</Card.Root>
