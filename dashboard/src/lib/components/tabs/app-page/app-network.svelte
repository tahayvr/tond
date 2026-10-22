<!--
  What an app's containers may connect to: open, an allowlist, or nothing.
  Enforced by the server firewall (TOND-EGRESS); previews (pull-request code)
  have their own policy, "none" by default. Editing is for admins.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { getAppEgress, updateAppEgress, type AppEgress, type EgressPolicy } from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { docsUrl } from '$lib/docs';
	import { cn } from '$lib/utils';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import AlertIcon from '~icons/ri/alert-line';
	import ShieldIcon from '~icons/ri/shield-check-line';

	interface Props {
		appId: string;
		appName: string;
		/** Compose apps have no previews. */
		sourceType: string;
		/** The user may change the policy (settings:write). */
		canEdit?: boolean;
	}

	let { appId, appName, sourceType, canEdit = false }: Props = $props();

	const POLICIES: { value: EgressPolicy; label: string; desc: string }[] = [
		{
			value: 'open',
			label: 'Open',
			desc: 'The app can connect anywhere on the internet.'
		},
		{
			value: 'restricted',
			label: 'Allowlist only',
			desc: 'Only the hosts and addresses listed below.'
		},
		{
			value: 'none',
			label: 'No internet',
			desc: 'Nothing outside the server: for apps that only talk to their databases.'
		}
	];
	const PREVIEW_LABELS: Record<EgressPolicy, string> = {
		none: 'No internet (recommended)',
		restricted: 'Allowlist only (same list)',
		open: 'Open'
	};
	const ALLOWLIST_EXAMPLE = ['api.stripe.com:443', '203.0.113.0/24', '198.51.100.7:5432'].join(
		'\n'
	);
	/** Strictest first, to tell tightening from loosening. */
	const RANK: Record<EgressPolicy, number> = { none: 0, restricted: 1, open: 2 };

	let egress = $state<AppEgress | null>(null);
	let loadError = $state('');
	let policy = $state<EgressPolicy>('open');
	let previewPolicy = $state<EgressPolicy>('none');
	let draft = $state('');

	function reset(e: AppEgress) {
		egress = e;
		policy = e.policy;
		previewPolicy = e.preview_policy;
		draft = e.allowlist.join('\n');
	}

	$effect(() => {
		const id = appId;
		untrack(() => {
			loadError = '';
			getAppEgress(id)
				.then(reset)
				.catch((e: any) => (loadError = e.message ?? "Couldn't load the network policy"));
		});
	});

	let entries = $derived(
		draft
			.split(/[\n,]+/)
			.map((l) => l.trim())
			.filter(Boolean)
	);
	let usesList = $derived(policy === 'restricted' || previewPolicy === 'restricted');
	let dirty = $derived(
		!!egress &&
			(policy !== egress.policy ||
				previewPolicy !== egress.preview_policy ||
				entries.join('\n') !== egress.allowlist.join('\n'))
	);
	let showPreviews = $derived(sourceType !== 'compose');

	const save = useAction(
		() =>
			updateAppEgress(appId, {
				policy,
				allowlist: entries,
				...(showPreviews ? { preview_policy: previewPolicy } : {})
			}),
		{
			success: (e) =>
				e.policy === 'open'
					? 'Saved. The app can connect anywhere.'
					: e.policy === 'none'
						? 'Saved. The app can no longer reach the internet.'
						: `Saved. The app can reach ${e.allowlist.length === 1 ? 'one destination' : `${e.allowlist.length} destinations`}.`,
			onSuccess: (e) => reset(e)
		}
	);

	async function handleSave() {
		if (!egress || save.pending) return;
		if (RANK[policy] < RANK[egress.policy]) {
			const ok = await confirmAction({
				title: `Restrict ${appName}'s outbound connections?`,
				description:
					'This applies to the running containers at once. Connections the app makes to anything else start failing right away.',
				details: [
					policy === 'none'
						? 'Only DNS and the app’s own Docker networks (linked databases, project members) stay reachable.'
						: `Only DNS, the app’s own Docker networks and ${entries.length === 1 ? 'one allowed destination' : `${entries.length} allowed destinations`} stay reachable.`
				],
				confirmLabel: 'Restrict'
			});
			if (!ok) return;
		}
		await save.run();
	}
</script>

<section class="grid gap-4" aria-labelledby="network-heading" data-testid="app-network">
	<div>
		<h2 id="network-heading" class="text-sm font-medium">Outbound connections</h2>
		<p class="mt-1 max-w-prose text-sm text-muted-foreground">
			What {appName}'s containers may connect to. DNS and the app's own Docker networks (linked
			databases, the rest of its project) always work, and so does traffic to the app.
			<a
				class="underline underline-offset-2"
				href={docsUrl('/security/isolation#outbound-connections')}>How it works</a
			>
		</p>
	</div>

	<InlineError message={loadError} title="Couldn't load the network policy" />

	{#if !egress && !loadError}
		<Skeleton class="h-24 w-full max-w-xl" />
	{:else if egress}
		{#if !egress.firewall_available}
			<div
				class="flex items-start gap-2 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn"
				role="note"
			>
				<AlertIcon class="mt-0.5 size-4 shrink-0" />
				<span
					>This server can't enforce a policy: it needs iptables with Docker's DOCKER-USER chain
					(nftables-only hosts and rootless Docker don't have it). Apps stay open.</span
				>
			</div>
		{:else if egress.warning}
			<div
				class="flex items-start gap-2 rounded-sm border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn"
				role="note"
			>
				<AlertIcon class="mt-0.5 size-4 shrink-0" />
				<span>{egress.warning}</span>
			</div>
		{/if}

		<div class="grid gap-2 sm:grid-cols-3" role="radiogroup" aria-label="Outbound policy">
			{#each POLICIES as p (p.value)}
				<button
					type="button"
					role="radio"
					aria-checked={policy === p.value}
					disabled={!canEdit ||
						save.pending ||
						(p.value !== 'open' && !egress.firewall_available && egress.policy === 'open')}
					class={cn(
						'rounded-md border p-3 text-left transition-colors hover:bg-accent/50 disabled:cursor-not-allowed disabled:opacity-60',
						policy === p.value && 'border-foreground bg-accent/40'
					)}
					onclick={() => (policy = p.value)}
				>
					<span class="block text-sm font-medium">{p.label}</span>
					<span class="mt-1 block text-xs text-muted-foreground">{p.desc}</span>
				</button>
			{/each}
		</div>

		{#if usesList}
			<div class="grid max-w-xl gap-2">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<Label for="egress-allowlist">Allowed destinations</Label>
					{#if egress.policy !== 'open' && !egress.warning && egress.firewall_available}
						<span class="flex items-center gap-1 text-xs text-success">
							<ShieldIcon class="size-3.5" /> Enforced by the server firewall
						</span>
					{/if}
				</div>
				<Textarea
					id="egress-allowlist"
					bind:value={draft}
					rows={4}
					placeholder={ALLOWLIST_EXAMPLE}
					class="font-mono text-sm"
					aria-describedby="egress-allowlist-help"
					disabled={!canEdit || save.pending}
				/>
				<p id="egress-allowlist-help" class="text-xs text-muted-foreground">
					One per line: an IPv4 address, a CIDR range or a hostname, with optional ports (<span
						class="font-mono">:443</span
					>
					or <span class="font-mono">:80,443</span>). Hostnames are resolved when saved and every 5
					minutes, so prefer CIDR ranges for services whose addresses change often.
				</p>
				{#if egress.hosts.length}
					<ul class="grid gap-1 text-xs" aria-label="Resolved hostnames">
						{#each egress.hosts as h (h.host)}
							<li class="flex flex-wrap gap-x-2">
								<span class="font-mono">{h.host}</span>
								{#if h.addresses.length}
									<span class="font-mono text-muted-foreground">→ {h.addresses.join(', ')}</span>
								{:else}
									<span class="text-warn">not resolved: blocked until it resolves</span>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}

		{#if showPreviews}
			<div class="grid max-w-xl gap-2 border-t pt-4">
				<Label for="egress-preview">Previews</Label>
				<Select.Root type="single" bind:value={previewPolicy} disabled={!canEdit || save.pending}>
					<Select.Trigger id="egress-preview" class="w-72" aria-label="Preview outbound policy">
						{PREVIEW_LABELS[previewPolicy]}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="none">{PREVIEW_LABELS.none}</Select.Item>
						<Select.Item value="restricted">{PREVIEW_LABELS.restricted}</Select.Item>
						<Select.Item value="open">{PREVIEW_LABELS.open}</Select.Item>
					</Select.Content>
				</Select.Root>
				<p class="text-xs text-muted-foreground">
					Previews run pull-request code, so by default they can't reach the internet: no
					exfiltrating whatever they get, no abuse from the server's address. Applies when a preview
					starts.
				</p>
				{#if egress.preview_warning}
					<p class="text-xs text-warn" role="note">{egress.preview_warning}</p>
				{/if}
			</div>
		{/if}

		{#if canEdit}
			<div class="flex flex-wrap items-center gap-3">
				<Button
					variant="outline"
					onclick={handleSave}
					disabled={!dirty || save.pending || (usesList && entries.length === 0)}
				>
					{save.pending ? 'Saving…' : 'Save'}
				</Button>
				<SuccessAlert message={save.success} ondismiss={save.clear} timeout={5000} />
			</div>
			{#if usesList && entries.length === 0}
				<p class="text-xs text-muted-foreground">
					Add at least one destination, or choose No internet.
				</p>
			{/if}
			<InlineError message={save.error} title="The policy wasn't saved" ondismiss={save.clear} />
		{:else}
			<p class="text-xs text-muted-foreground">Only admins can change the network policy.</p>
		{/if}
	{/if}
</section>
