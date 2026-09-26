<script lang="ts">
	import { listTokens, revokeToken, type ApiToken } from '$lib/api';
	import { relativeTime, tokenState, type TokenState } from '$lib/access';
	import { can } from '$lib/auth.svelte';
	import { cn } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import KeyIcon from '~icons/ri/key-2-line';
	import AddIcon from '~icons/ri/add-line';
	import TokenCreateDialog from './token-create-dialog.svelte';

	interface Props {
		initialTokens: ApiToken[];
	}

	let { initialTokens }: Props = $props();

	// Seeded from the route data; updated in place after create/revoke.
	let tokens = $derived<ApiToken[]>(initialTokens);
	let showAll = $state(false);
	let showInactive = $state(false);
	let loading = $state(false);
	let error = $state('');
	let createOpen = $state(false);
	let confirmToken = $state<ApiToken | null>(null);
	let revoking = $state(false);
	let revokeError = $state('');

	const canSeeAll = $derived(can('tokens:admin'));
	const visible = $derived(
		tokens.filter((t) => showInactive || ['active', 'expiring'].includes(tokenState(t)))
	);
	const hiddenCount = $derived(tokens.length - visible.length);

	const stateBadge: Record<TokenState, { label: string; class: string }> = {
		active: { label: 'Active', class: 'border-success/40 text-success' },
		expiring: { label: 'Expires soon', class: 'border-warn/40 text-warn' },
		expired: { label: 'Expired', class: 'text-muted-foreground' },
		revoked: { label: 'Revoked', class: 'text-muted-foreground' }
	};

	async function reload() {
		loading = true;
		error = '';
		try {
			tokens = await listTokens(showAll);
		} catch (e: any) {
			error = e.message;
		} finally {
			loading = false;
		}
	}

	async function handleRevoke() {
		if (!confirmToken) return;
		revoking = true;
		revokeError = '';
		try {
			const updated = await revokeToken(confirmToken.id);
			tokens = tokens.map((t) => (t.id === updated.id ? updated : t));
			confirmToken = null;
		} catch (e: any) {
			revokeError = e.message;
		} finally {
			revoking = false;
		}
	}

	function expiryText(t: ApiToken) {
		if (t.revoked_at) return `revoked ${relativeTime(t.revoked_at)}`;
		if (!t.expires_at) return 'never expires';
		const past = Date.parse(t.expires_at) <= Date.now();
		return `${past ? 'expired' : 'expires'} ${relativeTime(t.expires_at)}`;
	}

	function reachText(t: ApiToken) {
		const parts = [];
		if (t.app_ids.length) parts.push(`${t.app_ids.length} app${t.app_ids.length === 1 ? '' : 's'}`);
		if (t.project_ids.length)
			parts.push(`${t.project_ids.length} project${t.project_ids.length === 1 ? '' : 's'}`);
		return parts.length ? `limited to ${parts.join(' and ')}` : 'all apps';
	}
</script>

<section class="space-y-4">
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div class="max-w-xl">
			<h3 class="font-semibold">API tokens</h3>
			<p class="text-sm text-muted-foreground">
				For CI, scripts and agents. Each token has its own scopes, apps and expiry, and can be
				revoked on its own.
			</p>
		</div>
		<Button size="sm" onclick={() => (createOpen = true)}>
			<AddIcon class="mr-1 size-4" /> New token
		</Button>
	</div>

	<div class="flex flex-wrap items-center gap-4 text-sm text-muted-foreground">
		{#if canSeeAll}
			<label class="flex items-center gap-2">
				<Checkbox
					checked={showAll}
					onCheckedChange={(v) => {
						showAll = v === true;
						void reload();
					}}
				/>
				Everyone's tokens
			</label>
		{/if}
		<label class="flex items-center gap-2">
			<Checkbox checked={showInactive} onCheckedChange={(v) => (showInactive = v === true)} />
			Show revoked and expired{hiddenCount > 0 && !showInactive ? ` (${hiddenCount})` : ''}
		</label>
	</div>

	{#if error}
		<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
			{error}
		</div>
	{/if}

	{#if loading}
		<p class="py-6 text-center text-sm text-muted-foreground">Loading…</p>
	{:else if visible.length === 0}
		<div class="rounded-lg border border-dashed border-border py-10 text-center">
			<KeyIcon class="mx-auto mb-2 size-8 text-muted-foreground/40" />
			<p class="font-medium">No active tokens</p>
			<p class="mt-1 text-sm text-muted-foreground">
				Create one for a CI pipeline, a script or an AI agent.
			</p>
		</div>
	{:else}
		<ul class="divide-y divide-border rounded-lg border border-border bg-card">
			{#each visible as t (t.id)}
				{@const state = tokenState(t)}
				<li
					class={cn(
						'flex flex-col gap-3 p-4 sm:flex-row sm:items-start sm:justify-between',
						(state === 'revoked' || state === 'expired') && 'opacity-60'
					)}
				>
					<div class="min-w-0 space-y-1">
						<div class="flex flex-wrap items-center gap-2">
							<span class="font-medium">{t.name}</span>
							<Badge variant="outline" class="text-[10px] uppercase">{t.kind}</Badge>
							<Badge variant="outline" class={cn('text-[10px]', stateBadge[state].class)}>
								{stateBadge[state].label}
							</Badge>
						</div>
						<p class="font-mono text-xs text-muted-foreground">{t.prefix}…</p>
						<p class="text-xs text-muted-foreground">
							{t.scopes.length} scope{t.scopes.length === 1 ? '' : 's'}
							<span class="hidden sm:inline"
								>({t.scopes.slice(0, 4).join(', ')}{t.scopes.length > 4 ? ', …' : ''})</span
							>
							· {reachText(t)}{t.allowed_ips.length ? ` · from ${t.allowed_ips.join(', ')}` : ''}
						</p>
						<p class="text-xs text-muted-foreground">
							Last used {relativeTime(t.last_used_at)}{t.last_used_ip
								? ` from ${t.last_used_ip}`
								: ''}
							· {expiryText(t)}
							{#if showAll}· owner {t.owner_label}{/if}
						</p>
					</div>
					{#if !t.revoked_at}
						<Button
							variant="outline"
							size="sm"
							class="shrink-0 self-start text-destructive hover:text-destructive"
							onclick={() => {
								revokeError = '';
								confirmToken = t;
							}}
						>
							Revoke
						</Button>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>

<TokenCreateDialog
	bind:open={createOpen}
	onCreated={(token) => {
		tokens = [token, ...tokens];
	}}
/>

<AlertDialog.Root
	open={confirmToken !== null}
	onOpenChange={(o) => {
		if (!o && !revoking) confirmToken = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Revoke “{confirmToken?.name}”?</AlertDialog.Title>
			<AlertDialog.Description>
				Anything using this token stops working immediately. This can't be undone; you can create a
				new token later.
			</AlertDialog.Description>
		</AlertDialog.Header>
		{#if revokeError}
			<div class="rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
				{revokeError}
			</div>
		{/if}
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={revoking}>Keep it</AlertDialog.Cancel>
			<Button variant="destructive" onclick={handleRevoke} disabled={revoking}>
				{revoking ? 'Revoking…' : 'Revoke token'}
			</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
