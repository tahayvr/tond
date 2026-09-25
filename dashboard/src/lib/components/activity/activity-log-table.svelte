<script lang="ts">
	import { goto } from '$app/navigation';
	import { cn } from '$lib/utils';
	import Activity from '~icons/ri/pulse-line';
	import type { AuditLogEntry, App } from '$lib/api';

	interface Props {
		entries: AuditLogEntry[];
		apps: App[];
	}

	let { entries, apps }: Props = $props();

	function appName(appId: string | null): string {
		if (!appId) return '—';
		return apps.find((a) => a.id === appId)?.name ?? appId.slice(0, 8);
	}

	function actionColor(action: string): string {
		switch (action) {
			case 'delete':
				return 'text-destructive';
			case 'create':
			case 'deploy':
				return 'text-success';
			case 'stop':
				return 'text-warn';
			default:
				return 'text-muted-foreground';
		}
	}
</script>

<div class="relative overflow-hidden rounded-lg border border-border bg-card">
	<div
		class="pointer-events-none absolute inset-0 opacity-30"
		style="background-image: 
			linear-gradient(to right, rgba(255,255,255,0.03) 1px, transparent 1px),
			linear-gradient(to bottom, rgba(255,255,255,0.03) 1px, transparent 1px);
		background-size: 40px 40px;"
	></div>

	<div class="relative">
		<div class="flex items-center gap-2 border-b border-border p-4">
			<Activity class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				EVENT_LOG ({entries.length} ENTRIES)
			</span>
		</div>

		<table class="w-full text-sm">
			<thead>
				<tr class="border-b border-border bg-muted/50">
					<th
						class="px-4 py-3 text-left font-mono text-[10px] font-medium tracking-wider text-muted-foreground/70 uppercase"
					>
						Time
					</th>
					<th
						class="px-4 py-3 text-left font-mono text-[10px] font-medium tracking-wider text-muted-foreground/70 uppercase"
					>
						Action
					</th>
					<th
						class="px-4 py-3 text-left font-mono text-[10px] font-medium tracking-wider text-muted-foreground/70 uppercase"
					>
						Unit
					</th>
					<th
						class="px-4 py-3 text-left font-mono text-[10px] font-medium tracking-wider text-muted-foreground/70 uppercase"
					>
						Operator
					</th>
					<th
						class="px-4 py-3 text-left font-mono text-[10px] font-medium tracking-wider text-muted-foreground/70 uppercase"
					>
						Detail
					</th>
				</tr>
			</thead>
			<tbody>
				{#each entries as entry (entry.id)}
					<tr class="border-b border-border/50 last:border-0 hover:bg-muted/50">
						<td class="px-4 py-3 font-mono text-[10px] whitespace-nowrap text-muted-foreground/60">
							{new Date(entry.created_at).toLocaleString()}
						</td>
						<td class="px-4 py-3">
							<span class={cn('font-mono text-[10px] uppercase', actionColor(entry.action))}>
								{entry.action}
							</span>
						</td>
						<td class="px-4 py-3 font-mono text-[10px]">
							{#if entry.app_id}
								<button
									onclick={() => goto(`/apps/${entry.app_id}`)}
									class="text-muted-foreground hover:text-foreground hover:underline"
								>
									{entry.app_name ?? appName(entry.app_id)}
								</button>
							{:else}
								<span class="text-muted-foreground/60">{entry.app_name ?? '—'}</span>
							{/if}
						</td>
						<td class="px-4 py-3 font-mono text-[10px] text-muted-foreground/60">{entry.actor}</td>
						<td class="max-w-xs truncate px-4 py-3 font-mono text-[10px] text-muted-foreground/60">
							{entry.detail ?? '—'}
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
