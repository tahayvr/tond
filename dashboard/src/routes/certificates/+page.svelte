<script lang="ts">
	import { listCertificates, retryCertificate, type Certificate } from '$lib/api';
	import { can } from '$lib/auth.svelte';
	import { certificateProblem, sortCertificates } from '$lib/certificates';
	import { useAction } from '$lib/action.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import CertificateRow from '$lib/components/certificates/certificate-row.svelte';
	import { cn } from '$lib/utils';
	import LockIcon from '~icons/ri/lock-line';
	import ShieldCheck from '~icons/ri/shield-check-line';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	/** While something is being requested or retried, refresh this often. */
	const BUSY_POLL_MS = 15_000;
	/** Otherwise just keep "days left" and retry countdowns current. */
	const IDLE_TICK_MS = 60_000;

	let certificates = $state<Certificate[]>([]);
	let now = $state(Date.now());
	let loadError = $state('');

	$effect(() => {
		certificates = data.certificates as Certificate[];
		now = Date.now();
	});

	let problemsOnly = $derived(data.problemsOnly as boolean);
	let sorted = $derived(sortCertificates(certificates, now));
	let problemCount = $derived(sorted.filter((c) => certificateProblem(c, now) !== null).length);
	let shown = $derived(
		problemsOnly ? sorted.filter((c) => certificateProblem(c, now) !== null) : sorted
	);
	let busy = $derived(
		certificates.some((c) => c.status === 'pending' || c.status === 'failed' || c.last_error)
	);

	async function refresh() {
		try {
			certificates = await listCertificates();
			loadError = '';
		} catch (e: any) {
			loadError = e.message;
		} finally {
			now = Date.now();
		}
	}

	const retry = useAction(
		async (domain: string) => {
			await retryCertificate(domain);
			await refresh();
		},
		{
			key: (domain) => domain,
			success: (_r, domain) =>
				`Retry queued for ${domain}. Tond is requesting a certificate now; this page updates on its own.`
		}
	);

	$effect(() => {
		const ms = busy ? BUSY_POLL_MS : IDLE_TICK_MS;
		return pollWhileVisible(() => (busy ? refresh() : (now = Date.now())), ms, {
			immediate: false
		});
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});

	const FILTERS = [
		{ id: 'all', label: 'All', href: '/certificates' },
		{ id: 'problems', label: 'Needs attention', href: '/certificates?filter=problems' }
	] as const;
</script>

<div class="space-y-6">
	<PageHeader
		title="Certificates"
		description="HTTPS certificates for the dashboard, apps and previews. Tond requests and renews them automatically."
	/>

	<SuccessAlert message={retry.success} ondismiss={retry.clear} />
	<InlineError message={retry.error} ondismiss={retry.clear} />
	<InlineError message={loadError} ondismiss={() => (loadError = '')} />

	{#if certificates.length > 0}
		<nav aria-label="Filter certificates" class="flex flex-wrap items-center gap-2">
			{#each FILTERS as filter (filter.id)}
				{@const active = (filter.id === 'problems') === problemsOnly}
				<a
					href={filter.href}
					data-sveltekit-noscroll
					data-sveltekit-replacestate
					aria-current={active ? 'page' : undefined}
					class={cn(
						'inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm transition-colors',
						active
							? 'border-foreground bg-foreground text-background'
							: 'text-muted-foreground hover:bg-muted hover:text-foreground'
					)}
				>
					{filter.label}
					<span
						class={cn(
							'rounded-sm px-1.5 text-xs tabular-nums',
							active ? 'bg-background/20' : 'bg-muted',
							filter.id === 'problems' && problemCount > 0 && !active && 'bg-warn/15 text-warn'
						)}>{filter.id === 'problems' ? problemCount : certificates.length}</span
					>
				</a>
			{/each}
		</nav>
	{/if}

	{#if certificates.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-dashed border-border px-4 py-16 text-center"
		>
			<LockIcon class="mb-3 size-10 text-muted-foreground" aria-hidden="true" />
			<p class="font-medium">No certificates yet</p>
			<p class="mt-1 max-w-md text-sm text-muted-foreground">
				Set a dashboard domain in Settings or give an app a domain. Tond requests a certificate as
				soon as the domain is routed.
			</p>
		</div>
	{:else if shown.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-success/40 bg-success/5 px-4 py-12 text-center"
		>
			<ShieldCheck class="mb-3 size-10 text-success" aria-hidden="true" />
			<p class="font-medium">All certificates are healthy</p>
			<p class="mt-1 text-sm text-muted-foreground">
				Nothing is failing or close to expiry. <a href="/certificates" class="underline">Show all</a
				>
			</p>
		</div>
	{:else}
		<ul class="divide-y rounded-lg border bg-card" aria-label="Certificates">
			{#each shown as cert (cert.domain)}
				<CertificateRow
					{cert}
					{now}
					canWrite={can('apps:write')}
					retrying={retry.isPending(cert.domain)}
					onRetry={(domain) => retry.run(domain)}
				/>
			{/each}
		</ul>
		<p class="text-xs text-muted-foreground">
			Failed requests are retried automatically, waiting longer after each failure (from 1 minute up
			to 6 hours). "Retry now" skips the wait once you've fixed DNS or the firewall.
		</p>
	{/if}
</div>
