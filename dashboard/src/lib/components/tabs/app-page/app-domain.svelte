<script lang="ts" module>
	import type { DnsCheckStatus } from '$lib/api';

	/** Domains that never resolve publicly (the no-dashboard-domain fallback). */
	export function isPlaceholderDomain(domain: string | null | undefined): boolean {
		return !!domain && (domain === 'localhost' || domain.endsWith('.localhost'));
	}

	/** Short heading for a DNS check status. */
	export function dnsHeadline(status: DnsCheckStatus): string {
		switch (status) {
			case 'ok':
				return 'DNS points at this server';
			case 'mismatch':
				return 'DNS points somewhere else';
			case 'missing':
				return 'No DNS record yet';
			default:
				return "DNS can't be confirmed";
		}
	}

	/** The automatic address an app gets on its next deploy. */
	export function automaticAddress(appName: string, appsDomain: string | null): string | null {
		return appsDomain ? `${appName}.${appsDomain}` : null;
	}
</script>

<script lang="ts">
	// Domain and TLS card: the exact DNS record to create, live DNS and
	// certificate checks with one-click re-check/retry, and — for apps without
	// a domain — the choice between the automatic address and a custom domain.
	import { untrack } from 'svelte';
	import { checkDomain, getSettings, type App, type DomainCheck } from '$lib/api';
	import { pollWhileVisible } from '$lib/polling';
	import { relativeTime } from '$lib/deploy-view';
	import { cn, safeUrl } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';
	import GlobeIcon from '~icons/ri/global-line';
	import OkIcon from '~icons/ri/checkbox-circle-fill';
	import WarnIcon from '~icons/ri/error-warning-line';
	import InfoIcon from '~icons/ri/information-line';
	import RefreshIcon from '~icons/ri/refresh-line';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';
	import ExternalIcon from '~icons/ri/external-link-line';

	interface Props {
		app: App;
		/** Save a new domain (empty string is not allowed by the API). */
		onUpdate: (domain: string) => Promise<void>;
		/** Bumped after a save so the certificate status re-fetches. */
		certRefreshKey?: number;
	}

	let { app, onUpdate, certRefreshKey = 0 }: Props = $props();

	/** Re-check DNS this often while it isn't pointing here yet. */
	const DNS_POLL_MS = 30_000;

	let editing = $state(false);
	let domainInput = $state('');
	let saving = $state(false);
	let saveError = $state('');

	let check = $state<DomainCheck | null>(null);
	let checking = $state(false);
	let checkError = $state('');
	let now = $state(Date.now());
	let copied = $state('');
	let certKey = $state(0);

	let appsDomain = $state<string | null>(null);

	let domain = $derived(app.domain?.trim() || null);
	let placeholder = $derived(isPlaceholderDomain(domain));
	let appUrl = $derived(domain && !placeholder ? safeUrl(`https://${domain}`) : undefined);
	let automatic = $derived(automaticAddress(app.name, appsDomain));
	let isAutomatic = $derived(
		!!domain && (check ? check.under_apps_domain : !!automatic && domain === automatic)
	);

	async function runCheck() {
		if (!domain || placeholder) return;
		checking = true;
		checkError = '';
		try {
			check = await checkDomain(domain);
			if (check.apps_domain) appsDomain = check.apps_domain;
		} catch (e: any) {
			checkError = e.message ?? 'The DNS check failed';
		} finally {
			checking = false;
			now = Date.now();
		}
	}

	async function handleCheckAgain() {
		certKey += 1;
		await runCheck();
	}

	// Check when the domain changes; keep checking while DNS isn't right yet.
	$effect(() => {
		const target = domain;
		check = null;
		if (!target || isPlaceholderDomain(target)) return;
		return pollWhileVisible(
			() =>
				untrack(() => {
					if (check?.domain === target && check.dns.status === 'ok') {
						now = Date.now();
						return;
					}
					return runCheck();
				}),
			DNS_POLL_MS
		);
	});

	// Without a domain, find out what the automatic address would be.
	$effect(() => {
		if (domain) return;
		getSettings()
			.then((s) => {
				const apps = (s.apps_domain ?? s.dashboard_domain ?? '').trim();
				appsDomain = apps || null;
			})
			.catch(() => (appsDomain = null));
	});

	function startEditing() {
		domainInput = domain ?? '';
		saveError = '';
		editing = true;
	}

	async function handleSave(e?: Event) {
		e?.preventDefault();
		const value = domainInput.trim().toLowerCase().replace(/\.$/, '');
		if (!value) {
			saveError = 'Enter a domain like app.example.com.';
			return;
		}
		if (!/^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*[a-z0-9]$/.test(value)) {
			saveError = `"${value}" isn't a valid domain name.`;
			return;
		}
		saving = true;
		saveError = '';
		try {
			await onUpdate(value);
			editing = false;
		} catch (err: any) {
			saveError = err.message ?? 'Could not save the domain';
		} finally {
			saving = false;
		}
	}

	async function copy(value: string, key: string) {
		try {
			await navigator.clipboard.writeText(value);
			copied = key;
			setTimeout(() => {
				if (copied === key) copied = '';
			}, 1500);
		} catch {
			copied = '';
		}
	}
</script>

{#snippet recordTable(records: DomainCheck['records'], caption: string)}
	<div class="overflow-x-auto rounded-md border">
		<table class="w-full text-sm">
			<caption class="sr-only">{caption}</caption>
			<thead>
				<tr class="border-b bg-muted/40 text-left text-xs text-muted-foreground">
					<th class="px-3 py-2 font-medium">Type</th>
					<th class="px-3 py-2 font-medium">Name</th>
					<th class="px-3 py-2 font-medium">Value</th>
					<th class="w-10 px-2 py-2"><span class="sr-only">Copy</span></th>
				</tr>
			</thead>
			<tbody>
				{#each records as r (r.type + r.name + r.value)}
					<tr class="border-b last:border-0">
						<td class="px-3 py-2 font-mono text-xs">{r.type}</td>
						<td class="px-3 py-2">
							<div class="flex items-center gap-1">
								<code class="text-xs">{r.name}</code>
								<Button
									variant="ghost"
									size="icon-sm"
									class="size-6"
									onclick={() => copy(r.name, `name-${r.host}-${r.type}`)}
									aria-label="Copy name {r.name}"
								>
									{#if copied === `name-${r.host}-${r.type}`}<CheckIcon
											class="size-3.5"
										/>{:else}<CopyIcon class="size-3.5" />{/if}
								</Button>
							</div>
						</td>
						<td class="px-3 py-2"><code class="text-xs">{r.value}</code></td>
						<td class="px-2 py-2">
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7"
								onclick={() => copy(r.value, `value-${r.host}-${r.type}`)}
								aria-label="Copy value {r.value}"
							>
								{#if copied === `value-${r.host}-${r.type}`}<CheckIcon
										class="size-4"
									/>{:else}<CopyIcon class="size-4" />{/if}
							</Button>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{/snippet}

{#snippet editForm()}
	<form class="flex flex-col gap-2 sm:flex-row sm:items-end" onsubmit={handleSave}>
		<div class="grid flex-1 gap-1.5">
			<Label for="domain-input">Custom domain</Label>
			<Input
				id="domain-input"
				type="text"
				bind:value={domainInput}
				placeholder="app.example.com"
				autocomplete="off"
				spellcheck={false}
				disabled={saving}
			/>
		</div>
		<div class="flex gap-2">
			<Button type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save domain'}</Button>
			{#if domain}
				<Button variant="ghost" onclick={() => (editing = false)} disabled={saving}>Cancel</Button>
			{/if}
		</div>
	</form>
	{#if saveError}
		<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
			{saveError}
		</p>
	{/if}
{/snippet}

<div class="space-y-4 pt-4">
	<section class="space-y-4 rounded-md border p-4" aria-label="Domain">
		{#if domain}
			<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
				<div class="flex min-w-0 items-center gap-3">
					<GlobeIcon class="size-5 shrink-0 text-muted-foreground" />
					<div class="min-w-0">
						<div class="flex flex-wrap items-center gap-2">
							{#if appUrl}
								<a
									href={appUrl}
									target="_blank"
									rel="noreferrer"
									class="truncate font-medium underline-offset-2 hover:underline">{domain}</a
								>
							{:else}
								<span class="truncate font-medium">{domain}</span>
							{/if}
							<Badge variant="outline">{isAutomatic ? 'Automatic address' : 'Custom domain'}</Badge>
						</div>
						<p class="text-xs text-muted-foreground">
							{isAutomatic
								? 'Every app gets an address under the apps domain.'
								: 'Traffic for this domain goes to the app.'}
						</p>
					</div>
				</div>
				<div class="flex shrink-0 gap-2">
					{#if appUrl}
						<Button href={appUrl} target="_blank" rel="noreferrer" variant="ghost" size="sm">
							<ExternalIcon /> Open
						</Button>
					{/if}
					{#if !editing}
						<Button variant="outline" size="sm" onclick={startEditing}>Change domain</Button>
					{/if}
				</div>
			</div>
			{#if editing}
				{@render editForm()}
			{/if}
		{:else}
			<div class="space-y-1">
				<h3 class="font-medium">This app has no address yet</h3>
				<p class="text-sm text-muted-foreground">Choose how people reach it.</p>
			</div>
			<div class="grid gap-3 md:grid-cols-2">
				<div class="space-y-1.5 rounded-md border p-3">
					<p class="text-sm font-medium">Automatic address</p>
					{#if automatic}
						<p class="text-sm text-muted-foreground">
							Deploy the app and it's served at <code class="text-foreground">{automatic}</code>.
							One wildcard DNS record for <code>*.{appsDomain}</code> covers every app.
						</p>
					{:else}
						<p class="text-sm text-muted-foreground">
							Set a dashboard domain in <a href="/settings" class="underline underline-offset-2"
								>Settings</a
							> first; apps then get an address under it automatically.
						</p>
					{/if}
				</div>
				<div class="space-y-2 rounded-md border p-3">
					<p class="text-sm font-medium">Your own domain</p>
					<p class="text-sm text-muted-foreground">
						Use a domain you own. Tond shows the DNS record to create and gets a certificate.
					</p>
					{#if editing}
						{@render editForm()}
					{:else}
						<Button variant="outline" size="sm" onclick={startEditing}>Add a domain</Button>
					{/if}
				</div>
			</div>
		{/if}
	</section>

	{#if domain && placeholder}
		<div class="flex items-start gap-2 rounded-md border bg-muted/30 p-3 text-sm">
			<InfoIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
			<p class="text-muted-foreground">
				<code>{domain}</code> is a local placeholder and can't be reached from the internet. Set a
				dashboard domain in <a href="/settings" class="underline underline-offset-2">Settings</a> so apps
				get real addresses, or add your own domain.
			</p>
		</div>
	{:else if domain}
		<section class="space-y-3 rounded-md border p-4" aria-label="DNS">
			<div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
				<div class="flex min-w-0 items-start gap-2">
					{#if check?.dns.status === 'ok' && check.dns.stray.length === 0}
						<OkIcon class="mt-0.5 size-4 shrink-0 text-success" />
					{:else if check && check.dns.status !== 'inconclusive'}
						<WarnIcon class="mt-0.5 size-4 shrink-0 text-warn" />
					{:else}
						<InfoIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
					{/if}
					<div class="min-w-0 space-y-0.5">
						<p class="text-sm font-medium">
							{check ? dnsHeadline(check.dns.status) : checking ? 'Checking DNS…' : 'DNS'}
						</p>
						{#if check}
							<p class="text-sm break-words text-muted-foreground">{check.dns.message}</p>
						{/if}
					</div>
				</div>
				<div class="flex shrink-0 items-center gap-2">
					{#if check}
						<span class="text-xs text-muted-foreground">
							Checked {relativeTime(check.checked_at, now)}
						</span>
					{/if}
					<Button variant="outline" size="sm" onclick={handleCheckAgain} disabled={checking}>
						<RefreshIcon class={cn(checking && 'animate-spin')} />
						{checking ? 'Checking…' : 'Check again'}
					</Button>
				</div>
			</div>

			{#if checkError}
				<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
					{checkError}
				</p>
			{/if}

			{#if check}
				{#if check.records.length === 0}
					<p class="text-sm text-muted-foreground">
						Tond couldn't determine this server's public IP address. Point the domain at the
						server's public IP with an A record (and AAAA for IPv6).
					</p>
				{:else if check.dns.status !== 'ok' || check.dns.stray.length > 0}
					{#if isAutomatic && check.wildcard_records.length > 0}
						<p class="text-sm">
							Create this record at your DNS provider{check.zone ? ` for ${check.zone}` : ''}; it
							covers every app under <code>{check.apps_domain}</code>:
						</p>
						{@render recordTable(check.wildcard_records, 'Wildcard DNS records for all apps')}
						<p class="text-xs text-muted-foreground">Or add a record for this app only:</p>
					{:else}
						<p class="text-sm">
							Create {check.records.length === 1 ? 'this record' : 'these records'} at your DNS provider{check.zone
								? ` for ${check.zone}`
								: ''}:
						</p>
					{/if}
					{@render recordTable(check.records, 'DNS records to create')}
					{#if check.dns.resolved.length > 0}
						<p class="text-xs text-muted-foreground">
							Now resolves to <code>{check.dns.resolved.join(', ')}</code>{check.dns.expected.length
								? `; this server is ${check.dns.expected.join(', ')}`
								: ''}. DNS changes usually show up within minutes; Tond re-checks every 30 seconds.
						</p>
					{/if}
				{:else}
					<details class="text-sm">
						<summary class="cursor-pointer text-muted-foreground select-none">
							Show the DNS record
						</summary>
						<div class="mt-2">
							{@render recordTable(
								isAutomatic && check.wildcard_records.length
									? check.wildcard_records
									: check.records,
								'DNS records'
							)}
						</div>
					</details>
				{/if}
			{/if}
		</section>

		<section class="space-y-2" aria-label="HTTPS certificate">
			<h3 class="text-sm font-medium">HTTPS</h3>
			<CertificateStatus {domain} refreshKey={certRefreshKey + certKey} />
		</section>
	{/if}
</div>
