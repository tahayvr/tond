<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import type { SessionInfo } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Monitor from '~icons/ri/computer-line';
	import User from '~icons/ri/user-line';

	interface Props {
		sessions: SessionInfo[];
		loading: boolean;
		error: string;
		revokingId: string | null;
		revokingAll: boolean;
		onRevoke: (id: string) => void;
		onRevokeAll: () => void;
	}

	let { sessions, loading, error, revokingId, revokingAll, onRevoke, onRevokeAll }: Props =
		$props();

	function formatDate(iso: string | null) {
		if (!iso) return '—';
		return new Date(iso).toLocaleString(undefined, {
			month: 'short',
			day: 'numeric',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function shortAgent(ua: string | null) {
		if (!ua) return 'Unknown';
		if (ua.includes('tond-cli')) return 'tond CLI';
		if (ua.includes('Firefox')) return 'Firefox';
		if (ua.includes('Chrome')) return 'Chrome';
		if (ua.includes('Safari')) return 'Safari';
		return ua.slice(0, 40);
	}

	const hasOtherSessions = $derived(sessions.some((s) => !s.is_current));
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-4 flex items-center justify-between">
		<div class="flex items-center gap-2">
			<Monitor class="h-4 w-4 text-muted-foreground" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
				ACTIVE_SESSIONS
			</span>
		</div>
		{#if hasOtherSessions}
			<Button
				variant="outline"
				size="sm"
				onclick={onRevokeAll}
				disabled={revokingAll}
				class="text-xs"
			>
				{revokingAll ? 'Signing out…' : 'Sign out everywhere'}
			</Button>
		{/if}
	</div>

	<InlineError message={error} />

	{#if loading}
		<div
			class="py-8 text-center font-mono text-[11px] tracking-widest text-muted-foreground uppercase"
		>
			LOADING...
		</div>
	{:else if sessions.length === 0}
		<div
			class="py-8 text-center font-mono text-[11px] tracking-widest text-muted-foreground uppercase"
		>
			NO_ACTIVE_SESSIONS
		</div>
	{:else}
		<div class="space-y-2">
			{#each sessions as session (session.id)}
				<div
					class="flex items-start justify-between gap-4 rounded-md border border-border p-3 {session.is_current
						? 'bg-muted/40'
						: ''}"
				>
					<div class="min-w-0 flex-1 space-y-1">
						<div class="flex items-center gap-2">
							<User class="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
							<span class="truncate font-mono text-xs">
								{shortAgent(session.user_agent)}
							</span>
							{#if session.is_current}
								<Badge variant="secondary" class="font-mono text-[9px] uppercase">
									this session
								</Badge>
							{/if}
						</div>
						<div class="font-mono text-[10px] text-muted-foreground">
							{session.ip_address ?? 'unknown IP'} · last used {formatDate(session.last_used_at)} · expires
							{formatDate(session.expires_at)}
						</div>
					</div>

					{#if !session.is_current}
						<Button
							variant="ghost"
							size="sm"
							onclick={() => onRevoke(session.id)}
							disabled={revokingId === session.id}
							class="shrink-0 text-xs text-muted-foreground hover:text-destructive"
						>
							{revokingId === session.id ? '…' : 'Revoke'}
						</Button>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
