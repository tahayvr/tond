<script lang="ts">
	// DNS providers: API access Tond uses to publish ACME DNS-01 challenge
	// records. Unlocks one wildcard certificate for every app and preview, and
	// certificates for domains behind a CDN. Tokens are write-only.
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import SavedSecretField from '$lib/components/secrets/saved-secret-field.svelte';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { confirmAction } from '$lib/confirm.svelte';
	import { can } from '$lib/auth.svelte';
	import { formatRelativePast } from '$lib/time';
	import { cn } from '$lib/utils';
	import {
		createDnsProvider,
		deleteDnsProvider,
		listDnsProviders,
		testDnsProvider,
		updateDnsProvider,
		type DnsProvider,
		type DnsProviderKind,
		type DnsProviderTestResult
	} from '$lib/api';
	import { PROVIDER_HELP, parseZones, providerFor } from '$lib/domains';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import DnsIcon from '~icons/ri/server-line';
	import OkIcon from '~icons/ri/checkbox-circle-fill';
	import InfoIcon from '~icons/ri/information-line';
	import WarnIcon from '~icons/ri/error-warning-line';
	import AddIcon from '~icons/ri/add-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const KINDS: DnsProviderKind[] = ['cloudflare', 'hetzner', 'digitalocean', 'route53'];

	// Starts from the route data; refreshed in place after every change.
	let providers = $derived<DnsProvider[]>(data.providers as DnsProvider[]);
	let appsDomain = $derived((data.appsDomain as string) ?? '');
	let previewDomain = $derived((data.previewDomain as string) ?? '');
	let canWrite = $derived(can('settings:write'));
	let now = $state(Date.now());

	let error = $state('');
	let success = $state('');

	// Add form
	let adding = $state(false);
	let addOpen = $state(false);
	let kind = $state<DnsProviderKind>('cloudflare');
	let name = $state('');
	let zonesInput = $state('');
	let token = $state('');
	let keyId = $state('');
	let addError = $state('');
	let help = $derived(PROVIDER_HELP[kind]);

	// Edit form
	let editingId = $state<string | null>(null);
	let editName = $state('');
	let editZones = $state('');
	let editToken = $state('');
	let editReplacing = $state(false);
	let editKeyId = $state('');
	let saving = $state(false);
	let editError = $state('');

	// Test / delete
	let busyId = $state<string | null>(null);
	let tests = $state<Record<string, DnsProviderTestResult>>({});

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});

	/** The wildcards Tond can order, and who covers them. */
	let wildcards = $derived(
		[
			{ purpose: 'Every app', base: appsDomain, setting: 'apps domain' },
			{ purpose: 'Every preview', base: previewDomain, setting: 'preview domain' }
		]
			.filter((w) => w.base)
			.map((w) => ({ ...w, covered: providerFor(providers, w.base) }))
	);

	async function refresh() {
		providers = await listDnsProviders();
		now = Date.now();
	}

	function openAdd() {
		addOpen = true;
		editingId = null;
		kind = 'cloudflare';
		name = PROVIDER_HELP.cloudflare.label;
		const suggestion = wildcards.find((w) => !w.covered)?.base ?? '';
		zonesInput = suggestion;
		token = '';
		keyId = '';
		addError = '';
	}

	function chooseKind(k: DnsProviderKind) {
		if (!name.trim() || name === PROVIDER_HELP[kind].label) name = PROVIDER_HELP[k].label;
		kind = k;
	}

	async function handleAdd(e: Event) {
		e.preventDefault();
		const zones = parseZones(zonesInput);
		if (!name.trim()) return void (addError = 'Give the provider a name.');
		if (zones.length === 0) {
			return void (addError = 'List the zone the provider hosts, e.g. example.com.');
		}
		if (!token.trim()) return void (addError = `Paste the ${help.tokenLabel.toLowerCase()}.`);
		if (help.needsKeyId && !keyId.trim()) return void (addError = 'Enter the access key id.');
		adding = true;
		addError = '';
		try {
			const created = await createDnsProvider({
				name: name.trim(),
				kind,
				zones,
				token: token.trim(),
				config: help.needsKeyId ? { access_key_id: keyId.trim() } : {}
			});
			token = '';
			addOpen = false;
			await refresh();
			success = `${created.name} added. Tond now requests certificates for ${created.zones.join(', ')} through it; use Test to check the token.`;
		} catch (err: any) {
			addError = err.message ?? 'Could not add the provider';
		} finally {
			adding = false;
		}
	}

	function openEdit(p: DnsProvider) {
		addOpen = false;
		editingId = p.id;
		editName = p.name;
		editZones = p.zones.join(', ');
		editToken = '';
		editReplacing = false;
		editKeyId = p.config.access_key_id ?? '';
		editError = '';
	}

	async function handleSave(e: Event, p: DnsProvider) {
		e.preventDefault();
		const zones = parseZones(editZones);
		if (zones.length === 0) return void (editError = 'List at least one zone.');
		saving = true;
		editError = '';
		try {
			await updateDnsProvider(p.id, {
				name: editName.trim(),
				zones,
				...(editReplacing && editToken.trim() ? { token: editToken.trim() } : {}),
				...(PROVIDER_HELP[p.kind].needsKeyId ? { config: { access_key_id: editKeyId.trim() } } : {})
			});
			editingId = null;
			await refresh();
			success = `${editName.trim()} saved.`;
		} catch (err: any) {
			editError = err.message ?? 'Could not save the provider';
		} finally {
			saving = false;
		}
	}

	async function handleTest(p: DnsProvider) {
		busyId = p.id;
		error = '';
		try {
			tests = { ...tests, [p.id]: await testDnsProvider(p.id) };
			await refresh();
		} catch (err: any) {
			error = err.message ?? 'The test failed';
		} finally {
			busyId = null;
		}
	}

	async function handleDelete(p: DnsProvider) {
		await confirmAction({
			title: `Remove ${p.name}?`,
			description: `Tond stops using ${p.name} for ${p.zones.join(', ')}. Existing certificates keep working until they expire; renewals fall back to HTTP-01, which can't renew wildcard certificates.`,
			details: wildcards
				.filter((w) => w.covered?.provider.id === p.id)
				.map((w) => `*.${w.base} can't be renewed; its hosts get certificates of their own`),
			confirmLabel: 'Remove provider',
			pendingLabel: 'Removing…',
			destructive: true,
			onconfirm: async () => {
				await deleteDnsProvider(p.id);
				await refresh();
				success = `${p.name} removed.`;
			}
		});
	}
</script>

<PageTitle title="DNS providers · Integrations" />

<div class="space-y-8">
	<div class="flex h-14 items-center gap-4">
		<a
			href="/integrations"
			class="text-muted-foreground hover:text-foreground"
			aria-label="Back to integrations"
		>
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<DnsIcon class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">DNS providers</h1>
		</div>
	</div>

	<p class="max-w-2xl text-sm text-muted-foreground">
		Give Tond API access to your DNS so it can prove it controls a domain with a TXT record (ACME
		DNS-01). One wildcard certificate then covers every app and every preview instead of one
		certificate per host, and domains behind a CDN get certificates too. Tond only adds and removes
		<code>_acme-challenge</code> records; tokens are stored encrypted and never shown again.
	</p>

	<SuccessAlert message={success} ondismiss={() => (success = '')} />
	<InlineError message={error} ondismiss={() => (error = '')} />

	<section class="space-y-3" aria-label="Wildcard certificates">
		<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
			Wildcard certificates
		</h2>
		{#if wildcards.length === 0}
			<p
				class="flex items-start gap-2 rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground"
			>
				<InfoIcon class="mt-0.5 size-4 shrink-0" />
				<span>
					Set an apps domain or a preview domain in <a
						href="/settings"
						class="underline underline-offset-2">Settings</a
					> to use wildcard certificates.
				</span>
			</p>
		{:else}
			<ul class="divide-y rounded-lg border bg-card">
				{#each wildcards as w (w.base)}
					<li class="flex items-start gap-3 p-3 text-sm">
						{#if w.covered}
							<OkIcon class="mt-0.5 size-4 shrink-0 text-success" />
						{:else}
							<InfoIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
						{/if}
						<div class="min-w-0">
							<p class="font-medium"><code>*.{w.base}</code> · {w.purpose}</p>
							<p class="text-muted-foreground">
								{#if w.covered}
									Issued and renewed through {w.covered.provider.name} (zone {w.covered.zone}).
									Status on the
									<a href="/certificates" class="underline underline-offset-2">Certificates</a> page.
								{:else}
									Not covered: each {w.purpose === 'Every app' ? 'app' : 'preview'} gets its own certificate.
									Add a provider that hosts the {w.setting} ({w.base}) to use one wildcard.
								{/if}
							</p>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<section class="space-y-3" aria-label="Providers">
		<div class="flex items-center justify-between gap-2">
			<h2 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
				Providers
			</h2>
			{#if canWrite && !addOpen}
				<Button size="sm" variant="outline" onclick={openAdd}><AddIcon /> Add provider</Button>
			{/if}
		</div>

		{#if addOpen}
			<form class="space-y-4 rounded-lg border bg-card p-4" onsubmit={handleAdd}>
				<div class="space-y-1.5">
					<p class="text-sm font-medium">Provider</p>
					<div class="flex flex-wrap gap-2" role="group" aria-label="DNS provider">
						{#each KINDS as k (k)}
							<Button
								type="button"
								size="sm"
								variant={kind === k ? 'default' : 'outline'}
								aria-pressed={kind === k}
								onclick={() => chooseKind(k)}>{PROVIDER_HELP[k].label}</Button
							>
						{/each}
					</div>
				</div>
				<div class="rounded-md border bg-muted/30 p-3 text-sm">
					<p class="font-medium">Create a token that can only edit DNS</p>
					<ol class="mt-1 list-decimal space-y-1 pl-5 text-muted-foreground">
						{#each help.steps as step (step)}
							<li>{step}</li>
						{/each}
					</ol>
					<a
						href={help.docs}
						target="_blank"
						rel="noreferrer"
						class="mt-2 inline-flex items-center gap-1 text-xs underline underline-offset-2"
						>{help.label} documentation <ExternalIcon class="size-3" /></a
					>
				</div>
				<div class="grid gap-4 sm:grid-cols-2">
					<div class="grid content-start gap-1.5">
						<Label for="dns-name">Name</Label>
						<Input id="dns-name" bind:value={name} disabled={adding} />
					</div>
					<div class="grid content-start gap-1.5">
						<Label for="dns-zones">Zones</Label>
						<Input
							id="dns-zones"
							bind:value={zonesInput}
							placeholder="example.com"
							autocomplete="off"
							spellcheck={false}
							disabled={adding}
						/>
						<p class="text-xs text-muted-foreground">
							The domains this account hosts, comma separated. Subdomains are included.
						</p>
					</div>
					{#if help.needsKeyId}
						<div class="grid content-start gap-1.5">
							<Label for="dns-key-id">Access key id</Label>
							<Input
								id="dns-key-id"
								bind:value={keyId}
								placeholder="AKIA…"
								autocomplete="off"
								spellcheck={false}
								disabled={adding}
							/>
						</div>
					{/if}
					<div class="grid content-start gap-1.5">
						<Label for="dns-token">{help.tokenLabel}</Label>
						<SecretInput
							id="dns-token"
							bind:value={token}
							copyable={false}
							autocomplete="off"
							disabled={adding}
						/>
						<p class="text-xs text-muted-foreground">Stored encrypted and never shown again.</p>
					</div>
				</div>
				<InlineError message={addError} ondismiss={() => (addError = '')} />
				<div class="flex gap-2">
					<Button type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add provider'}</Button>
					<Button type="button" variant="ghost" onclick={() => (addOpen = false)} disabled={adding}
						>Cancel</Button
					>
				</div>
			</form>
		{/if}

		{#if providers.length === 0 && !addOpen}
			<div
				class="flex flex-col items-center justify-center rounded-lg border border-dashed px-4 py-12 text-center"
			>
				<DnsIcon class="mb-3 size-8 text-muted-foreground" aria-hidden="true" />
				<p class="font-medium">No DNS provider yet</p>
				<p class="mt-1 max-w-md text-sm text-muted-foreground">
					Without one, Tond validates every domain over HTTP (port 80) and requests one certificate
					per host. That works; a provider is only needed for wildcards and CDN-fronted domains.
				</p>
			</div>
		{:else}
			<ul class="space-y-3" aria-label="DNS providers">
				{#each providers as p (p.id)}
					{@const test = tests[p.id]}
					<li class="space-y-3 rounded-lg border bg-card p-4" data-testid="dns-provider">
						<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
							<div class="min-w-0 space-y-1">
								<p class="font-medium">
									{p.name}
									<span class="font-normal text-muted-foreground"
										>· {PROVIDER_HELP[p.kind].label}</span
									>
								</p>
								<div class="flex flex-wrap gap-1">
									{#each p.zones as z (z)}<Badge variant="outline">{z}</Badge>{/each}
								</div>
								<p class="text-xs text-muted-foreground">
									{p.last_used_at
										? `Last published a record ${formatRelativePast(p.last_used_at, now)}`
										: 'Not used yet'}
									· token saved
								</p>
							</div>
							{#if canWrite}
								<div class="flex flex-wrap gap-2">
									<Button
										size="sm"
										variant="outline"
										onclick={() => handleTest(p)}
										disabled={busyId === p.id}
									>
										{busyId === p.id ? 'Testing…' : 'Test'}
									</Button>
									<Button size="sm" variant="ghost" onclick={() => openEdit(p)}>Edit</Button>
									<Button size="sm" variant="ghost" onclick={() => handleDelete(p)}>Remove</Button>
								</div>
							{/if}
						</div>
						{#if p.last_error && !test}
							<p
								class="flex items-start gap-2 rounded-md border border-warn/40 bg-warn/10 p-2 text-sm"
							>
								<WarnIcon class="mt-0.5 size-4 shrink-0 text-warn" />
								<span class="break-words">{p.last_error}</span>
							</p>
						{/if}
						{#if test}
							<ul
								class={cn(
									'space-y-1 rounded-md border p-2 text-sm',
									test.ok ? 'border-success/40 bg-success/5' : 'border-warn/40 bg-warn/10'
								)}
								aria-label="Test result for {p.name}"
							>
								{#each test.zones as z (z.zone)}
									<li class="flex items-start gap-2">
										{#if z.ok}<OkIcon class="mt-0.5 size-4 shrink-0 text-success" />{:else}<WarnIcon
												class="mt-0.5 size-4 shrink-0 text-warn"
											/>{/if}
										<span class="break-words">{z.message}</span>
									</li>
								{/each}
							</ul>
						{/if}
						{#if editingId === p.id}
							<form
								class="grid gap-3 border-t pt-3 sm:grid-cols-2"
								onsubmit={(e) => handleSave(e, p)}
							>
								<div class="grid content-start gap-1.5">
									<Label for="edit-name-{p.id}">Name</Label>
									<Input id="edit-name-{p.id}" bind:value={editName} disabled={saving} />
								</div>
								<div class="grid content-start gap-1.5">
									<Label for="edit-zones-{p.id}">Zones</Label>
									<Input id="edit-zones-{p.id}" bind:value={editZones} disabled={saving} />
								</div>
								{#if PROVIDER_HELP[p.kind].needsKeyId}
									<div class="grid content-start gap-1.5">
										<Label for="edit-key-{p.id}">Access key id</Label>
										<Input id="edit-key-{p.id}" bind:value={editKeyId} disabled={saving} />
									</div>
								{/if}
								<div class="grid content-start gap-1.5">
									<Label for="edit-token-{p.id}">{PROVIDER_HELP[p.kind].tokenLabel}</Label>
									<SavedSecretField
										id="edit-token-{p.id}"
										saved={p.token_set}
										bind:value={editToken}
										bind:replacing={editReplacing}
										disabled={saving}
									/>
								</div>
								<div class="sm:col-span-2">
									<InlineError message={editError} ondismiss={() => (editError = '')} />
								</div>
								<div class="flex gap-2 sm:col-span-2">
									<Button type="submit" size="sm" disabled={saving}
										>{saving ? 'Saving…' : 'Save'}</Button
									>
									<Button
										type="button"
										size="sm"
										variant="ghost"
										onclick={() => (editingId = null)}
										disabled={saving}>Cancel</Button
									>
								</div>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>
</div>
