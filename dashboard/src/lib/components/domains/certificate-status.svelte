<script lang="ts" module>
	import type { Certificate } from '$lib/api';

	export type CertificateView =
		| { kind: 'none' }
		| { kind: 'active'; expiresAt: string; renewalError: string | null }
		| { kind: 'requesting' }
		| { kind: 'retrying'; failed: boolean; error: string; retryIn: string | null };

	/** "12 min", "3 h", "in a moment" — how long until `iso`, relative to `now`. */
	export function formatRetryIn(iso: string | null, now: number): string | null {
		if (!iso) return null;
		const ms = new Date(iso).getTime() - now;
		if (Number.isNaN(ms)) return null;
		if (ms <= 60_000) return 'less than a minute';
		const minutes = Math.round(ms / 60_000);
		if (minutes < 60) return `${minutes} min`;
		const hours = Math.floor(minutes / 60);
		const rest = minutes % 60;
		return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
	}

	/**
	 * Plain-language explanation of a certificate error, or null when the
	 * message is already readable (e.g. Tond's own DNS pre-check).
	 */
	export function explainCertError(raw: string, domain: string): string | null {
		const e = raw.toLowerCase();
		if (e.startsWith('dns for ')) return null;
		if (e.includes('ratelimited') || e.includes('too many')) {
			return `Let's Encrypt's rate limit for ${domain} was reached. Nothing to fix on your side if DNS is right: Tond keeps retrying with backoff and it clears on its own.`;
		}
		if (e.includes(':caa') || e.includes('caa record')) {
			return `A CAA record on ${domain} doesn't allow Let's Encrypt. Add a CAA record 0 issue "letsencrypt.org", or remove the CAA records.`;
		}
		if (e.includes('rejectedidentifier')) {
			return `Let's Encrypt won't issue a certificate for ${domain}. Use a public domain name you own.`;
		}
		if (
			e.includes(':dns') ||
			e.includes('nxdomain') ||
			e.includes('no valid a records') ||
			e.includes('servfail')
		) {
			return `${domain} doesn't resolve yet. Add an A (and AAAA, if you use IPv6) record pointing at this server. DNS changes can take a few minutes to spread.`;
		}
		if (
			e.includes(':connection') ||
			e.includes('timeout during connect') ||
			e.includes('connection refused') ||
			e.includes('timed out')
		) {
			return `Let's Encrypt couldn't connect to port 80 on ${domain}. Make sure your cloud firewall allows inbound ports 80 and 443, and that the DNS record points at this server.`;
		}
		if (e.includes('unauthorized') || e.includes('acme-challenge')) {
			return `Let's Encrypt reached a server for ${domain}, but it wasn't this one. Check that the domain's A/AAAA records point at this server and that no CDN proxy or other server answers on port 80.`;
		}
		return null;
	}

	/** Decide what to show for a domain's certificate row (or its absence). */
	export function certificateView(cert: Certificate | undefined, now: number): CertificateView {
		if (!cert) return { kind: 'none' };
		if (cert.status === 'active') {
			return { kind: 'active', expiresAt: cert.expires_at, renewalError: cert.last_error };
		}
		if (!cert.last_error) return { kind: 'requesting' };
		return {
			kind: 'retrying',
			failed: cert.status === 'failed',
			error: cert.last_error,
			retryIn: formatRetryIn(cert.next_retry_at, now)
		};
	}
</script>

<script lang="ts">
	import { listCertificates, retryCertificate } from '$lib/api';
	import { findCertificate } from '$lib/domains';
	import { pollWhileVisible } from '$lib/polling';
	import { Button } from '$lib/components/ui/button/index.js';
	import { cn } from '$lib/utils';
	import LockIcon from '~icons/ri/lock-line';
	import WarningIcon from '~icons/ri/error-warning-line';
	import TimeIcon from '~icons/ri/time-line';
	import RefreshIcon from '~icons/ri/refresh-line';

	interface Props {
		/** Domain whose certificate to show; nothing renders when empty. */
		domain: string | null;
		/** Bump to re-fetch (e.g. after the domain is saved). */
		refreshKey?: number;
	}

	let { domain, refreshKey = 0 }: Props = $props();

	/** Re-check while a certificate is pending (the proxy tries every ~30 s). */
	const POLL_MS = 15_000;

	let cert = $state<Certificate | undefined>(undefined);
	/** Set when a wildcard certificate serves the domain. */
	let coveredBy = $state<string | null>(null);
	let loaded = $state(false);
	let now = $state(Date.now());
	let retrying = $state(false);
	let error = $state('');

	let trimmed = $derived((domain ?? '').trim());
	let view = $derived(certificateView(cert, now));

	async function load(target: string) {
		try {
			const certs = await listCertificates();
			if (target !== trimmed) return; // domain changed meanwhile
			({ cert, coveredBy } = findCertificate(certs, target));
			error = '';
		} catch (e: any) {
			error = e.message;
		} finally {
			loaded = true;
			now = Date.now();
		}
	}

	async function handleRetry() {
		if (!trimmed) return;
		retrying = true;
		error = '';
		try {
			await retryCertificate(trimmed);
			await load(trimmed);
		} catch (e: any) {
			error = e.message;
		} finally {
			retrying = false;
		}
	}

	$effect(() => {
		const target = trimmed;
		void refreshKey;
		cert = undefined;
		coveredBy = null;
		loaded = false;
		if (!target) return;
		load(target);
		// Paused while the tab is hidden; refreshes as soon as it is shown again.
		return pollWhileVisible(
			() => {
				if (cert?.status !== 'active') return load(target);
				now = Date.now();
			},
			POLL_MS,
			{ immediate: false }
		);
	});
</script>

{#snippet technical(raw: string)}
	{#if explainCertError(raw, trimmed)}
		<details class="text-xs text-muted-foreground">
			<summary class="cursor-pointer select-none">Technical details</summary>
			<p class="mt-1 font-mono break-words">{raw}</p>
		</details>
	{/if}
{/snippet}

{#if trimmed && loaded}
	<div
		class={cn(
			'rounded-md border p-3 text-sm',
			view.kind === 'active' && !view.renewalError && 'border-success/40 bg-success/5',
			view.kind === 'retrying' && 'border-warn/50 bg-warn/10',
			(view.kind === 'requesting' || view.kind === 'none') && 'border-border bg-card',
			view.kind === 'active' && view.renewalError && 'border-warn/50 bg-warn/10'
		)}
		data-testid="certificate-status"
	>
		{#if view.kind === 'active'}
			<div class="flex items-start gap-2">
				<LockIcon class="mt-0.5 size-4 shrink-0 text-success" />
				<div class="space-y-1">
					<p class="font-medium">HTTPS active for {trimmed}</p>
					{#if cert?.cert_type === 'custom'}
						<p class="text-muted-foreground">
							Uploaded certificate{coveredBy ? ` (${coveredBy})` : ''}, valid until {new Date(
								view.expiresAt
							).toLocaleDateString()}. Tond doesn't renew uploaded certificates: upload a new one on
							the <a href="/certificates" class="underline underline-offset-2">Certificates</a> page before
							it expires. Expiry alerts still warn you.
						</p>
					{:else if coveredBy}
						<p class="text-muted-foreground">
							Served by the wildcard certificate <code class="text-foreground">{coveredBy}</code>,
							valid until {new Date(view.expiresAt).toLocaleDateString()}. Tond renews it
							automatically.
						</p>
					{:else}
						<p class="text-muted-foreground">
							Certificate valid until {new Date(view.expiresAt).toLocaleDateString()}. Tond renews
							it automatically 30 days before it expires{cert?.challenge === 'dns-01'
								? ' (validated through your DNS provider)'
								: ''}.
						</p>
					{/if}
					{#if view.renewalError}
						<p class="text-foreground">
							The last renewal attempt failed. {explainCertError(view.renewalError, trimmed) ??
								view.renewalError}
						</p>
						{@render technical(view.renewalError)}
					{/if}
				</div>
			</div>
		{:else if view.kind === 'retrying'}
			<div class="flex items-start gap-2">
				<WarningIcon class="mt-0.5 size-4 shrink-0 text-warn" />
				<div class="min-w-0 flex-1 space-y-1">
					<p class="font-medium">
						{view.failed ? 'Certificate request failed' : 'Certificate pending'} for {trimmed}
					</p>
					<p class="break-words text-foreground">
						{explainCertError(view.error, trimmed) ?? view.error}
					</p>
					{@render technical(view.error)}
					<p class="flex items-center gap-1 text-muted-foreground">
						<TimeIcon class="size-3.5" />
						{#if view.retryIn}
							Tond retries automatically in {view.retryIn}. Fixed the problem? Retry now.
						{:else}
							Tond retries automatically within a minute.
						{/if}
					</p>
				</div>
				<Button
					variant="outline"
					size="sm"
					class="shrink-0"
					onclick={handleRetry}
					disabled={retrying}
				>
					<RefreshIcon class={cn('size-4', retrying && 'animate-spin')} />
					{retrying ? 'Queuing…' : 'Retry now'}
				</Button>
			</div>
		{:else if view.kind === 'requesting'}
			<div class="flex items-start gap-2">
				<TimeIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
				<p class="text-muted-foreground">
					Certificate pending for {trimmed}: requesting one from Let's Encrypt. This usually takes
					under a minute once the domain's DNS points at this server.
				</p>
			</div>
		{:else}
			<div class="flex items-start gap-2">
				<TimeIcon class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
				<p class="text-muted-foreground">
					No certificate yet for {trimmed}. Tond requests one automatically once the domain is
					routed (the app is running, or the dashboard domain is saved).
				</p>
			</div>
		{/if}
	</div>
{/if}

{#if error}
	<div class="mt-2 rounded-md bg-destructive/10 p-2 text-sm text-destructive">{error}</div>
{/if}
