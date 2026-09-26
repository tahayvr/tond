<script lang="ts" module>
	export { dnsHeadline } from '$lib/components/domains/domain-dns-panel.svelte';

	/** Domains that never resolve publicly (the no-dashboard-domain fallback). */
	export function isPlaceholderDomain(domain: string | null | undefined): boolean {
		return !!domain && (domain === 'localhost' || domain.endsWith('.localhost'));
	}

	/** The automatic address an app gets on its next deploy. */
	export function automaticAddress(appName: string, appsDomain: string | null): string | null {
		return appsDomain ? `${appName}.${appsDomain}` : null;
	}
</script>

<script lang="ts">
	// Domains card: every domain of the app (primary, aliases, redirects) with
	// its DNS and HTTPS status at a glance, adding domains (optionally with the
	// www/apex counterpart redirecting), making one primary, turning one into
	// a redirect, removing one — and, for the selected domain, the exact DNS
	// records to create and the certificate status.
	import { untrack } from 'svelte';
	import {
		addAppDomain,
		addWwwPair,
		appReadiness,
		checkDomain,
		getSettings,
		listAppDomains,
		listCertificates,
		removeAppDomain,
		updateAppDomain,
		type App,
		type AppDomain,
		type Certificate,
		type DomainCheck
	} from '$lib/api';
	import { confirmAction } from '$lib/confirm.svelte';
	import {
		KIND_LABELS,
		domainSummary,
		findCertificate,
		redirectTargets,
		validateDomainInput,
		wwwCounterpart
	} from '$lib/domains';
	import { pollWhileVisible } from '$lib/polling';
	import { cn, safeUrl } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import CertificateStatus from '$lib/components/domains/certificate-status.svelte';
	import DomainDnsPanel from '$lib/components/domains/domain-dns-panel.svelte';
	import GlobeIcon from '~icons/ri/global-line';
	import WarnIcon from '~icons/ri/error-warning-line';
	import InfoIcon from '~icons/ri/information-line';
	import ExternalIcon from '~icons/ri/external-link-line';
	import DeleteIcon from '~icons/ri/delete-bin-line';
	import StarIcon from '~icons/ri/star-line';
	import RedirectIcon from '~icons/ri/corner-down-right-line';
	import AddIcon from '~icons/ri/add-line';

	interface Props {
		app: App;
		/** Bumped by the parent to re-fetch certificate status. */
		certRefreshKey?: number;
		/** The user may change domains (apps:write). */
		canEdit?: boolean;
		/** Called after any change (the parent reloads the app: its primary domain changed). */
		onChanged?: () => unknown;
	}

	let { app, certRefreshKey = 0, canEdit = true, onChanged }: Props = $props();

	/** Re-check the selected domain's DNS this often while it isn't right yet. */
	const DNS_POLL_MS = 30_000;

	let domains = $state<AppDomain[]>([]);
	let loaded = $state(false);
	let loadError = $state('');
	let certs = $state<Certificate[]>([]);
	let checks = $state<Record<string, DomainCheck>>({});
	let checking = $state<Record<string, boolean>>({});
	let checkError = $state('');
	let now = $state(Date.now());
	let certKey = $state(0);
	let selected = $state<string | null>(null);
	let appsDomain = $state<string | null>(null);

	// Add form
	let addOpen = $state(false);
	let domainInput = $state('');
	let mode = $state<'serve' | 'redirect'>('serve');
	let redirectTo = $state('');
	let redirectCode = $state<'308' | '301'>('308');
	let withWww = $state(false);
	let adding = $state(false);
	let addError = $state('');

	// Row actions
	let busyDomain = $state<string | null>(null);
	let rowError = $state('');
	let editingRedirect = $state<string | null>(null);
	let editTarget = $state('');
	let editCode = $state<'308' | '301'>('308');

	let primary = $derived(domains.find((d) => d.kind === 'primary') ?? null);
	let current = $derived(
		domains.find((d) => d.domain === selected) ?? primary ?? domains[0] ?? null
	);
	let currentDomain = $derived(current?.domain ?? null);
	let currentCheck = $derived(currentDomain ? (checks[currentDomain] ?? null) : null);
	let placeholder = $derived(isPlaceholderDomain(currentDomain));
	let warning = $derived(current?.warning ?? currentCheck?.warning ?? null);
	let targets = $derived(redirectTargets(domains));
	let automatic = $derived(automaticAddress(app.name, appsDomain));
	let parsedInput = $derived(domainInput.trim() ? validateDomainInput(domainInput) : null);
	let counterpart = $derived(
		parsedInput && 'domain' in parsedInput ? wwwCounterpart(parsedInput.domain) : null
	);

	async function refreshCerts() {
		try {
			certs = await listCertificates();
		} catch {
			// The status column just stays as it was; CertificateStatus shows errors.
		}
	}

	async function runCheck(domain: string) {
		if (isPlaceholderDomain(domain)) return;
		checking = { ...checking, [domain]: true };
		if (domain === currentDomain) checkError = '';
		try {
			const result = await checkDomain(domain);
			checks = { ...checks, [domain]: result };
			if (result.apps_domain) appsDomain = result.apps_domain;
		} catch (e: any) {
			if (domain === currentDomain) checkError = e.message ?? 'The DNS check failed';
		} finally {
			checking = { ...checking, [domain]: false };
			now = Date.now();
		}
	}

	/** One DNS check per domain for the status column (the selected one keeps polling). */
	function checkUnchecked() {
		for (const d of domains) {
			if (!checks[d.domain] && !checking[d.domain] && d.domain !== currentDomain) {
				runCheck(d.domain);
			}
		}
	}

	async function load() {
		try {
			const [list] = await Promise.all([listAppDomains(app.id), refreshCerts()]);
			domains = list;
			loadError = '';
		} catch (e: any) {
			loadError = e.message ?? "Couldn't load the app's domains";
		} finally {
			loaded = true;
		}
		checkUnchecked();
	}

	/** Every change returns the full list; keep the selection and tell the parent. */
	async function applied(list: AppDomain[], select?: string) {
		domains = list;
		if (select) selected = select;
		certKey += 1;
		await Promise.all([refreshCerts(), onChanged?.()]);
		checkUnchecked();
	}

	$effect(() => {
		void app.id;
		untrack(load);
	});

	// Keep checking the selected domain until DNS is right.
	$effect(() => {
		const target = currentDomain;
		if (!target || isPlaceholderDomain(target)) return;
		return pollWhileVisible(
			() =>
				untrack(() => {
					if (checks[target]?.dns.status === 'ok') {
						now = Date.now();
						return;
					}
					return runCheck(target);
				}),
			DNS_POLL_MS
		);
	});

	// Without domains, find out what the automatic address would be.
	$effect(() => {
		if (!loaded || domains.length > 0) return;
		getSettings()
			.then((s) => {
				// Apps never fall back to the dashboard's domain (same site).
				const apps = (s.apps_domain ?? '').trim();
				appsDomain = apps || null;
			})
			.catch(() => (appsDomain = null));
	});

	function openAdd() {
		addOpen = true;
		addError = '';
		domainInput = '';
		mode = 'serve';
		redirectTo = primary?.domain ?? targets[0] ?? '';
		redirectCode = '308';
		withWww = false;
	}

	async function handleAdd(e?: Event) {
		e?.preventDefault();
		const parsed = validateDomainInput(domainInput);
		if ('error' in parsed) {
			addError = parsed.error;
			return;
		}
		if (mode === 'redirect' && !redirectTo) {
			addError = 'Choose the domain to redirect to.';
			return;
		}
		adding = true;
		addError = '';
		try {
			let list: AppDomain[];
			if (mode === 'serve' && withWww) {
				// The entered name serves; it becomes primary unless the app already
				// has a custom primary domain.
				const makePrimary = !primary || primary.automatic;
				list = await addWwwPair(
					app.id,
					parsed.domain,
					parsed.domain.startsWith('www.') ? 'www' : 'apex',
					makePrimary
				);
			} else {
				list = await addAppDomain(app.id, {
					domain: parsed.domain,
					...(mode === 'redirect'
						? {
								kind: 'redirect' as const,
								redirect_to: redirectTo,
								redirect_code: Number(redirectCode) as 301 | 308
							}
						: {})
				});
			}
			addOpen = false;
			await applied(list, parsed.domain);
		} catch (err: any) {
			addError = err.message ?? 'Could not add the domain';
		} finally {
			adding = false;
		}
	}

	async function rowAction(domain: string, fn: () => Promise<AppDomain[]>) {
		busyDomain = domain;
		rowError = '';
		try {
			await applied(await fn());
		} catch (e: any) {
			rowError = e.message ?? 'That change failed';
		} finally {
			busyDomain = null;
		}
	}

	function handleMakePrimary(d: AppDomain) {
		return rowAction(d.domain, () => updateAppDomain(app.id, d.domain, { kind: 'primary' }));
	}

	function handleServe(d: AppDomain) {
		return rowAction(d.domain, () => updateAppDomain(app.id, d.domain, { kind: 'alias' }));
	}

	function startRedirect(d: AppDomain) {
		editingRedirect = d.domain;
		const options = redirectTargets(domains, d.domain);
		editTarget = d.redirect_to ?? primary?.domain ?? options[0] ?? '';
		if (editTarget === d.domain) editTarget = options[0] ?? '';
		editCode = d.redirect_code === 301 ? '301' : '308';
		rowError = '';
	}

	async function saveRedirect(d: AppDomain) {
		if (!editTarget) {
			rowError = 'Choose the domain to redirect to.';
			return;
		}
		await rowAction(d.domain, () =>
			updateAppDomain(app.id, d.domain, {
				kind: 'redirect',
				redirect_to: editTarget,
				redirect_code: Number(editCode) as 301 | 308
			})
		);
		if (!rowError) editingRedirect = null;
	}

	async function handleRemove(d: AppDomain) {
		const others = domains.filter((x) => x.domain !== d.domain && x.kind !== 'redirect');
		await confirmAction({
			title: `Remove ${d.domain}?`,
			description:
				d.kind === 'primary' && others.length
					? `${others[0].domain} becomes the primary domain.`
					: 'Visitors to this domain get a "no route" page. Its certificate is no longer renewed.',
			details:
				d.kind === 'primary' && !others.length
					? ['The app has no other domain, so it has no public address until you add one.']
					: undefined,
			confirmLabel: 'Remove domain',
			pendingLabel: 'Removing…',
			destructive: true,
			onconfirm: async () => {
				const list = await removeAppDomain(app.id, d.domain);
				if (selected === d.domain) selected = null;
				await applied(list);
			}
		});
	}

	function dnsChip(domain: string): { label: string; tone: string } {
		if (isPlaceholderDomain(domain)) return { label: 'Local only', tone: 'muted' };
		const c = checks[domain];
		if (!c) return { label: checking[domain] ? 'DNS…' : 'DNS', tone: 'muted' };
		if (c.dns.status === 'ok') return { label: 'DNS ok', tone: 'ok' };
		if (c.dns.status === 'inconclusive') return { label: 'DNS unconfirmed', tone: 'muted' };
		return { label: c.dns.status === 'missing' ? 'No DNS record' : 'DNS elsewhere', tone: 'warn' };
	}

	function httpsChip(domain: string): { label: string; tone: string } {
		const { cert, coveredBy } = findCertificate(certs, domain);
		if (!cert) return { label: 'No certificate', tone: 'muted' };
		if (cert.status === 'active') {
			if (cert.cert_type === 'custom') return { label: 'HTTPS · uploaded', tone: 'ok' };
			return { label: coveredBy ? 'HTTPS · wildcard' : 'HTTPS', tone: 'ok' };
		}
		if (cert.status === 'failed' || cert.last_error)
			return { label: 'HTTPS failing', tone: 'warn' };
		return { label: 'Requesting HTTPS', tone: 'muted' };
	}

	const CHIP_TONE: Record<string, string> = {
		ok: 'border-success/40 bg-success/10 text-success',
		warn: 'border-warn/50 bg-warn/10 text-warn',
		muted: 'text-muted-foreground'
	};
</script>

{#snippet chip(c: { label: string; tone: string })}
	<span
		class={cn('rounded-sm border px-1.5 py-0.5 text-[11px] whitespace-nowrap', CHIP_TONE[c.tone])}
		>{c.label}</span
	>
{/snippet}

{#snippet addForm()}
	<form class="space-y-3 rounded-md border bg-muted/20 p-3" onsubmit={handleAdd}>
		<div class="grid gap-1.5">
			<Label for="domain-input">Domain</Label>
			<Input
				id="domain-input"
				type="text"
				bind:value={domainInput}
				placeholder="app.example.com"
				autocomplete="off"
				spellcheck={false}
				disabled={adding}
			/>
		</div>
		{#if domains.length > 0}
			<div class="flex flex-wrap gap-2" role="group" aria-label="What the domain does">
				<Button
					type="button"
					size="sm"
					variant={mode === 'serve' ? 'default' : 'outline'}
					aria-pressed={mode === 'serve'}
					onclick={() => (mode = 'serve')}>Serves the app</Button
				>
				<Button
					type="button"
					size="sm"
					variant={mode === 'redirect' ? 'default' : 'outline'}
					aria-pressed={mode === 'redirect'}
					disabled={targets.length === 0}
					onclick={() => (mode = 'redirect')}>Redirects to another domain</Button
				>
			</div>
		{/if}
		{#if mode === 'redirect'}
			<div class="flex flex-col gap-2 sm:flex-row">
				<Select.Root type="single" bind:value={redirectTo}>
					<Select.Trigger class="w-full sm:w-72" aria-label="Redirect target">
						{redirectTo || 'Choose a domain'}
					</Select.Trigger>
					<Select.Content>
						{#each targets as t (t)}
							<Select.Item value={t}>{t}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<Select.Root type="single" bind:value={redirectCode}>
					<Select.Trigger class="w-full sm:w-56" aria-label="Redirect status">
						{redirectCode === '308' ? '308 Permanent (keeps method)' : '301 Moved permanently'}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="308">308 Permanent (keeps method)</Select.Item>
						<Select.Item value="301">301 Moved permanently</Select.Item>
					</Select.Content>
				</Select.Root>
			</div>
			<p class="text-xs text-muted-foreground">
				Tond answers the redirect itself (the app never sees these requests) and keeps the path and
				query: <code>/pricing?plan=pro</code> goes to the same path on the target.
			</p>
		{:else if counterpart}
			<label class="flex items-start gap-2 text-sm">
				<Checkbox bind:checked={withWww} aria-label="Also add {counterpart}, redirecting here" />
				<span>
					Also add <code>{counterpart}</code>, redirecting here
					<span class="block text-xs text-muted-foreground">
						Visitors who type either name end up on one address (308, path and query kept).
					</span>
				</span>
			</label>
		{/if}
		<InlineError message={addError} ondismiss={() => (addError = '')} />
		<div class="flex gap-2">
			<Button type="submit" disabled={adding}>{adding ? 'Adding…' : 'Add domain'}</Button>
			<Button variant="ghost" type="button" onclick={() => (addOpen = false)} disabled={adding}
				>Cancel</Button
			>
		</div>
	</form>
{/snippet}

<div class="space-y-4 pt-4">
	{#if appReadiness(app) === 'none'}
		<div class="flex items-start gap-2 rounded-md border bg-muted/30 p-4 text-sm">
			<InfoIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
			<p class="text-muted-foreground">
				{app.name} is a background worker: it serves no traffic, so it has no domain and Tond doesn't
				route to it. To give it an address, make it a web app under
				<a href="/apps/{app.id}?tab=settings&section=health" class="underline underline-offset-2"
					>Health</a
				>.
			</p>
		</div>
	{:else if loaded && loadError}
		<InlineError message={loadError} />
	{:else if loaded && domains.length === 0}
		<section class="space-y-4 rounded-md border p-4" aria-label="Domains">
			<div class="space-y-1">
				<h2 class="font-medium">This app has no address yet</h2>
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
							No apps domain is set. Set one in <a
								href="/settings"
								class="underline underline-offset-2">Settings</a
							> (a domain on a different site from the dashboard) and apps get an address under it automatically
							— or add your own domain.
						</p>
					{/if}
				</div>
				<div class="space-y-2 rounded-md border p-3">
					<p class="text-sm font-medium">Your own domain</p>
					<p class="text-sm text-muted-foreground">
						Use a domain you own. Tond shows the DNS record to create and gets a certificate.
					</p>
					{#if addOpen}
						{@render addForm()}
					{:else if canEdit}
						<Button variant="outline" size="sm" onclick={openAdd}>Add a domain</Button>
					{:else}
						<p class="text-xs text-muted-foreground">Ask an admin or deployer to add one.</p>
					{/if}
				</div>
			</div>
		</section>
	{:else if loaded}
		<section class="space-y-3 rounded-md border p-4" aria-label="Domains">
			<div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
				<div>
					<h2 class="font-medium">Domains</h2>
					<p class="text-sm text-muted-foreground">
						Every domain here reaches {app.name}. Select one to see its DNS records and certificate.
					</p>
				</div>
				{#if canEdit && !addOpen}
					<Button variant="outline" size="sm" onclick={openAdd} class="shrink-0">
						<AddIcon /> Add domain
					</Button>
				{/if}
			</div>

			{#if addOpen}
				{@render addForm()}
			{/if}
			<InlineError message={rowError} ondismiss={() => (rowError = '')} />

			<ul class="divide-y rounded-md border" aria-label="App domains">
				{#each domains as d (d.domain)}
					{@const isCurrent = d.domain === currentDomain}
					{@const url = isPlaceholderDomain(d.domain) ? undefined : safeUrl(`https://${d.domain}`)}
					<li class={cn('p-3', isCurrent && 'bg-muted/40')} data-testid="domain-row">
						<div class="flex flex-col gap-2 md:flex-row md:items-center md:justify-between">
							<button
								type="button"
								class="flex min-w-0 flex-1 items-start gap-2 text-left"
								aria-pressed={isCurrent}
								aria-label="Show DNS and HTTPS for {d.domain}"
								onclick={() => (selected = d.domain)}
							>
								{#if d.kind === 'redirect'}
									<RedirectIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
								{:else}
									<GlobeIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
								{/if}
								<span class="min-w-0">
									<span class="flex flex-wrap items-center gap-1.5">
										<span class="truncate font-medium">{d.domain}</span>
										<Badge variant={d.kind === 'primary' ? 'default' : 'outline'}
											>{KIND_LABELS[d.kind]}</Badge
										>
										{#if d.automatic}<Badge variant="secondary">Automatic</Badge>{/if}
									</span>
									<span class="block text-xs text-muted-foreground">{domainSummary(d)}</span>
								</span>
							</button>
							<div class="flex flex-wrap items-center gap-1.5 pl-6 md:justify-end md:pl-0">
								<div class="flex flex-wrap items-center gap-1.5">
									{@render chip(dnsChip(d.domain))}
									{@render chip(httpsChip(d.domain))}
									{#if url && d.kind !== 'redirect'}
										<Button
											href={url}
											target="_blank"
											rel="noreferrer"
											variant="ghost"
											size="icon-sm"
											aria-label="Open {d.domain}"
										>
											<ExternalIcon />
										</Button>
									{/if}
								</div>
								{#if canEdit}
									<div class="flex flex-wrap items-center gap-1">
										{#if d.kind === 'alias'}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => handleMakePrimary(d)}
												disabled={busyDomain !== null}
												aria-label="Make {d.domain} the primary domain"
											>
												<StarIcon />
												Make primary
											</Button>
										{/if}
										{#if d.kind === 'redirect'}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => handleServe(d)}
												disabled={busyDomain !== null}
												aria-label="Serve the app on {d.domain} instead of redirecting"
												>Serve instead</Button
											>
										{/if}
										{#if d.kind !== 'primary' && redirectTargets(domains, d.domain).length > 0}
											<Button
												variant="ghost"
												size="sm"
												onclick={() => startRedirect(d)}
												disabled={busyDomain !== null}
												aria-label="{d.kind === 'redirect'
													? 'Change the redirect of'
													: 'Redirect'} {d.domain}"
											>
												{d.kind === 'redirect' ? 'Edit redirect' : 'Redirect'}
											</Button>
										{/if}
										<Button
											variant="ghost"
											size="icon-sm"
											onclick={() => handleRemove(d)}
											disabled={busyDomain !== null}
											aria-label="Remove {d.domain}"
										>
											<DeleteIcon />
										</Button>
									</div>
								{/if}
							</div>
						</div>
						{#if editingRedirect === d.domain}
							<div
								class="mt-3 flex flex-col gap-2 rounded-md border bg-background p-3 sm:flex-row sm:items-center"
							>
								<span class="text-sm">Send visitors to</span>
								<Select.Root type="single" bind:value={editTarget}>
									<Select.Trigger class="w-full sm:w-64" aria-label="Redirect {d.domain} to">
										{editTarget || 'Choose a domain'}
									</Select.Trigger>
									<Select.Content>
										{#each redirectTargets(domains, d.domain) as t (t)}
											<Select.Item value={t}>{t}</Select.Item>
										{/each}
									</Select.Content>
								</Select.Root>
								<Select.Root type="single" bind:value={editCode}>
									<Select.Trigger class="w-full sm:w-28" aria-label="Redirect status">
										{editCode}
									</Select.Trigger>
									<Select.Content>
										<Select.Item value="308">308</Select.Item>
										<Select.Item value="301">301</Select.Item>
									</Select.Content>
								</Select.Root>
								<div class="flex gap-2">
									<Button size="sm" onclick={() => saveRedirect(d)} disabled={busyDomain !== null}>
										{busyDomain === d.domain ? 'Saving…' : 'Save'}
									</Button>
									<Button size="sm" variant="ghost" onclick={() => (editingRedirect = null)}
										>Cancel</Button
									>
								</div>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		</section>

		{#if current && placeholder}
			<div class="flex items-start gap-2 rounded-md border bg-muted/30 p-3 text-sm">
				<InfoIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
				<p class="text-muted-foreground">
					<code>{current.domain}</code> is a local placeholder and can't be reached from the
					internet. Set an apps domain in
					<a href="/settings" class="underline underline-offset-2">Settings</a> so apps get real addresses,
					or add your own domain.
				</p>
			</div>
		{:else if current}
			<h3 class="pt-2 text-sm font-medium">
				{current.domain}
				<span class="font-normal text-muted-foreground"
					>· {KIND_LABELS[current.kind].toLowerCase()}</span
				>
			</h3>
			{#if warning}
				<div
					class="flex items-start gap-2 rounded border border-warn/40 bg-warn/10 px-3 py-2 text-sm"
					role="note"
				>
					<WarnIcon class="mt-0.5 size-4 shrink-0 text-warn" />
					<div>
						<p class="font-medium text-warn">Same site as your dashboard</p>
						<p class="text-xs text-muted-foreground">{warning}</p>
					</div>
				</div>
			{/if}
			<DomainDnsPanel
				check={currentCheck}
				checking={!!checking[current.domain]}
				{checkError}
				{now}
				automatic={currentCheck ? currentCheck.under_apps_domain : current.automatic}
				onCheckAgain={() => {
					certKey += 1;
					if (currentDomain) runCheck(currentDomain);
				}}
			/>
			<section class="space-y-2" aria-label="HTTPS certificate">
				<h2 class="text-sm font-medium">HTTPS</h2>
				<CertificateStatus domain={current.domain} refreshKey={certRefreshKey + certKey} />
			</section>
		{/if}
	{/if}
</div>
