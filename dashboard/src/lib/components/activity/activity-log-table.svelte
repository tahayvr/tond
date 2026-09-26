<script lang="ts">
	import { cn } from '$lib/utils';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import UserIcon from '~icons/ri/user-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import TerminalIcon from '~icons/ri/terminal-box-line';
	import CpuIcon from '~icons/ri/cpu-line';
	import QuestionIcon from '~icons/ri/question-line';
	import ShieldIcon from '~icons/ri/shield-keyhole-line';
	import type { AuditLogEntry } from '$lib/api';
	import { actionLabel, actorLabel, isSecurityAction, relativeTime } from '$lib/access';

	interface Props {
		entries: AuditLogEntry[];
		/** Show only entries from this actor when their name is clicked */
		onFilterActor?: (actor: string) => void;
	}

	let { entries, onFilterActor }: Props = $props();

	const kindIcon = {
		user: UserIcon,
		token: KeyIcon,
		local_socket: TerminalIcon,
		system: CpuIcon,
		anonymous: QuestionIcon
	} as Record<string, typeof UserIcon>;

	function resourceHref(e: AuditLogEntry): string | null {
		if (!e.resource_id) return null;
		switch (e.resource_type) {
			case 'app':
				return `/apps/${e.resource_id}`;
			case 'database':
				return `/databases/${e.resource_id}`;
			case 'project':
				return `/projects/${e.resource_id}`;
			default:
				return null;
		}
	}

	function absolute(iso: string) {
		return new Date(iso).toLocaleString();
	}
</script>

<ol class="divide-y divide-border rounded-lg border border-border bg-card" aria-label="Activity">
	{#each entries as entry (entry.seq)}
		{@const Icon = kindIcon[entry.actor_kind] ?? QuestionIcon}
		{@const href = resourceHref(entry)}
		<li
			class={cn(
				'grid gap-1 px-4 py-3 text-sm sm:grid-cols-[8rem_1fr] sm:gap-4',
				entry.outcome === 'failure' && 'bg-destructive/5'
			)}
		>
			<time
				datetime={entry.created_at}
				title={absolute(entry.created_at)}
				class="text-xs text-muted-foreground sm:pt-0.5"
			>
				{relativeTime(entry.created_at)}
			</time>
			<div class="min-w-0 space-y-1">
				<p class="flex flex-wrap items-center gap-x-1.5 gap-y-1">
					<button
						type="button"
						class="inline-flex max-w-full items-center gap-1 font-medium hover:underline"
						title="Show only this actor"
						onclick={() => onFilterActor?.(entry.actor_label)}
					>
						<Icon class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
						<span class="truncate">{actorLabel(entry)}</span>
					</button>
					<span class="text-muted-foreground">{actionLabel(entry.action)}</span>
					{#if entry.resource_name}
						{#if href}
							<a {href} class="font-medium hover:underline">{entry.resource_name}</a>
						{:else}
							<span class="font-medium">{entry.resource_name}</span>
						{/if}
					{/if}
					{#if entry.outcome === 'failure'}
						<Badge variant="destructive" class="text-[10px]">failed</Badge>
					{:else if isSecurityAction(entry.action)}
						<ShieldIcon class="size-3.5 text-muted-foreground" aria-label="Security event" />
					{/if}
				</p>
				{#if entry.detail || entry.ip}
					<p class="truncate font-mono text-xs text-muted-foreground" title={entry.detail ?? ''}>
						{#if entry.detail}{entry.detail}{/if}
						{#if entry.ip}<span class="text-muted-foreground">
								{entry.detail ? ' · ' : ''}from {entry.ip}</span
							>{/if}
					</p>
				{/if}
			</div>
		</li>
	{/each}
</ol>
