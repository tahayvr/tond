<!--
  Who may connect to a public database (the allowlist, enforced in the
  server's firewall) and whether it serves TLS. Shown in the Network access
  card; editing is for admins.
-->
<script lang="ts">
	import { updateDatabase, type ManagedDatabaseDetail } from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { parseAllowlist } from '$lib/database-ops';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import AlertIcon from '~icons/ri/alert-line';
	import ShieldIcon from '~icons/ri/shield-check-line';
	import LockIcon from '~icons/ri/lock-2-line';

	interface Props {
		db: ManagedDatabaseDetail;
		canEdit: boolean;
		onchange: (db: ManagedDatabaseDetail) => void;
	}

	let { db, canEdit, onchange }: Props = $props();

	let draft = $derived(db.allowlist.join('\n'));

	let parsed = $derived(parseAllowlist(draft));
	let dirty = $derived(parsed.entries.join('\n') !== db.allowlist.join('\n'));

	const saveAllowlist = useAction(() => updateDatabase(db.id, { allowlist: parsed.entries }), {
		success: (d) =>
			d.allowlist.length
				? `Only ${d.allowlist.length === 1 ? 'this address' : `these ${d.allowlist.length} addresses`} can connect now.`
				: 'Every address can connect now.',
		onSuccess: (d) => onchange(d)
	});

	function handleTls(enable: boolean) {
		return confirmAction({
			title: enable ? `Serve TLS from ${db.name}?` : `Stop serving TLS from ${db.name}?`,
			description: enable
				? 'The database restarts with a certificate for the dashboard domain (a few seconds of downtime). Clients then connect with TLS; the public connection string asks for it.'
				: 'The database restarts without TLS (a few seconds of downtime). Clients that require TLS can no longer connect.',
			confirmLabel: enable ? 'Turn on TLS' : 'Turn off TLS',
			destructive: !enable,
			pendingLabel: 'Restarting…',
			onconfirm: async () => onchange(await updateDatabase(db.id, { tls: enable }))
		});
	}
</script>

<div class="space-y-4" data-testid="db-public-security">
	{#if db.access_warning}
		<div
			class="flex items-start gap-2 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn"
			role="note"
		>
			<AlertIcon class="mt-0.5 size-4 shrink-0" />
			<span>{db.access_warning}</span>
		</div>
	{/if}

	{#if db.public_access}
		<div class="grid gap-2">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<Label for="db-allowlist">Allowed addresses</Label>
				{#if db.allowlist_enforced}
					<span class="flex items-center gap-1 text-xs text-success">
						<ShieldIcon class="size-3.5" /> Enforced by the server firewall
					</span>
				{/if}
			</div>
			<Textarea
				id="db-allowlist"
				bind:value={draft}
				rows={3}
				placeholder="e.g. 203.0.113.7 or 198.51.100.0/24"
				class="max-w-md font-mono text-sm"
				aria-invalid={!!parsed.invalid}
				aria-describedby="db-allowlist-help"
				disabled={!canEdit || saveAllowlist.pending}
			/>
			{#if parsed.invalid}
				<p class="text-xs text-destructive">
					“{parsed.invalid}” isn't an IPv4 address or a CIDR range like 198.51.100.0/24.
				</p>
			{:else}
				<p id="db-allowlist-help" class="text-xs text-muted-foreground">
					One IPv4 address or CIDR range per line. Connections from anywhere else are dropped before
					they reach the database. Leave empty to allow every address.
				</p>
			{/if}
			{#if canEdit}
				<div class="flex flex-wrap items-center gap-3">
					<Button
						size="sm"
						variant="outline"
						disabled={!dirty || !!parsed.invalid || saveAllowlist.pending}
						onclick={() => saveAllowlist.run()}
					>
						{saveAllowlist.pending ? 'Saving…' : 'Save allowed addresses'}
					</Button>
					<SuccessAlert
						message={saveAllowlist.success}
						ondismiss={saveAllowlist.clear}
						timeout={5000}
					/>
				</div>
				<InlineError message={saveAllowlist.error} ondismiss={saveAllowlist.clear} />
			{/if}
		</div>
	{/if}

	{#if db.supports_tls}
		<div class="flex flex-wrap items-start justify-between gap-3 border-t pt-4">
			<div class="max-w-prose space-y-1">
				<p class="flex items-center gap-1.5 text-sm font-medium">
					<LockIcon class="size-4" /> TLS
					<span class="font-normal text-muted-foreground">· {db.tls ? 'On' : 'Off'}</span>
				</p>
				<p class="text-xs text-muted-foreground">
					{#if db.tls}
						Clients can connect with TLS (<span class="font-mono">sslmode=require</span>). Apps on
						the internal network may keep connecting without it.
					{:else}
						Encrypt connections from outside with a certificate for the dashboard domain (Let's
						Encrypt when available, self-signed otherwise).
					{/if}
				</p>
			</div>
			{#if canEdit}
				<Button size="sm" variant="outline" onclick={() => handleTls(!db.tls)}>
					{db.tls ? 'Turn off TLS…' : 'Turn on TLS…'}
				</Button>
			{/if}
		</div>
	{/if}
</div>
