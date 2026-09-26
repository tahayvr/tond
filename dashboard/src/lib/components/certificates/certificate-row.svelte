<!--
  CertificateRow — one certificate on the Certificates page: domain, what it
  serves, issuer, status, expiry, last renewal, and (when something is wrong)
  the plain-language reason, next automatic retry and "Retry now".
-->
<script lang="ts">
	import type { Certificate } from '$lib/api';
	import {
		canRetry,
		certificateBadge,
		certificateProblem,
		daysLeft,
		daysLeftLabel,
		expiryTone,
		hasIssuedCertificate,
		issuerLabel,
		targetLabel
	} from '$lib/certificates';
	import {
		explainCertError,
		formatRetryIn
	} from '$lib/components/domains/certificate-status.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { formatRelativePast } from '$lib/time';
	import { cn } from '$lib/utils';
	import RefreshIcon from '~icons/ri/refresh-line';
	import TimeIcon from '~icons/ri/time-line';

	let {
		cert,
		now,
		canWrite,
		retrying,
		onRetry
	}: {
		cert: Certificate;
		now: number;
		canWrite: boolean;
		retrying: boolean;
		onRetry: (domain: string) => void;
	} = $props();

	let badge = $derived(certificateBadge(cert, now));
	let problem = $derived(certificateProblem(cert, now));
	let issued = $derived(hasIssuedCertificate(cert));
	let days = $derived(issued ? daysLeft(cert.expires_at, now) : null);
	let tone = $derived(expiryTone(days));
	let target = $derived(targetLabel(cert.serves));
	let explained = $derived(cert.last_error ? explainCertError(cert.last_error, cert.domain) : null);
	let retryIn = $derived(formatRetryIn(cert.next_retry_at, now));

	const TONE_TEXT = {
		success: 'text-foreground',
		warning: 'text-warn',
		danger: 'text-destructive',
		progress: 'text-foreground',
		neutral: 'text-muted-foreground'
	} as const;
</script>

<li class="space-y-3 p-4" data-testid="certificate-row">
	<div
		class="grid grid-cols-2 items-start gap-3 md:grid-cols-[minmax(0,1fr)_11rem_15rem_8.5rem] md:items-center"
	>
		<div class="col-span-2 min-w-0 md:col-span-1">
			<p class="truncate font-mono text-sm font-medium" title={cert.domain}>{cert.domain}</p>
			<p class="truncate text-xs text-muted-foreground">
				{#if target.href}
					<a href={target.href} class="hover:text-foreground hover:underline">{target.label}</a>
				{:else}
					{target.label}
				{/if}
				{#if cert.issuer_kind !== 'unknown'}
					<span aria-hidden="true">&nbsp;·&nbsp;</span>{issuerLabel(cert.issuer_kind, cert.issuer)}
				{/if}
			</p>
		</div>

		<div>
			<StatusBadge status={badge.status} tone={badge.tone} label={badge.label} kind="certificate" />
		</div>

		<div class="text-sm">
			{#if issued}
				<p class={cn('font-medium tabular-nums', TONE_TEXT[tone])}>{daysLeftLabel(days)}</p>
				<p class="text-xs text-muted-foreground">
					{new Date(cert.expires_at).toLocaleDateString(undefined, { dateStyle: 'medium' })}
					{#if cert.last_renewed_at}
						<span aria-hidden="true">&nbsp;·&nbsp;</span>renewed {formatRelativePast(
							cert.last_renewed_at,
							now
						)}
					{/if}
				</p>
			{:else if cert.last_attempt_at}
				<p class="text-muted-foreground">No certificate yet</p>
				<p class="text-xs text-muted-foreground">
					Last attempt {formatRelativePast(cert.last_attempt_at, now)}
				</p>
			{:else}
				<p class="text-muted-foreground">Requesting…</p>
			{/if}
		</div>

		<div class="col-span-2 empty:hidden md:col-span-1 md:text-right">
			{#if canWrite && canRetry(cert, now)}
				<Button
					variant="outline"
					size="sm"
					onclick={() => onRetry(cert.domain)}
					disabled={retrying}
					aria-label={`Retry certificate for ${cert.domain} now`}
				>
					<RefreshIcon
						class={cn('size-4', retrying && 'animate-spin motion-reduce:animate-none')}
					/>
					{retrying ? 'Queuing…' : 'Retry now'}
				</Button>
			{/if}
		</div>
	</div>

	{#if problem === 'untrusted'}
		<p class="rounded-md border border-warn/40 bg-warn/10 p-3 text-sm">
			Issued by Let's Encrypt's staging environment, which browsers don't trust. Remove
			<code class="font-mono text-xs">TOND_ACME_STAGING</code> from tond-proxy's environment and restart
			it to get a trusted certificate.
		</p>
	{:else if cert.last_error && problem}
		<div
			class={cn(
				'space-y-1.5 rounded-md border p-3 text-sm',
				problem === 'failing' && cert.status === 'failed'
					? 'border-destructive/40 bg-destructive/5'
					: 'border-warn/40 bg-warn/10'
			)}
		>
			<p class="break-words">
				{#if issued}The last renewal attempt failed.&nbsp;{/if}{explained ?? cert.last_error}
			</p>
			{#if explained}
				<details class="text-xs text-muted-foreground">
					<summary class="cursor-pointer select-none">Technical details</summary>
					<p class="mt-1 font-mono break-words">{cert.last_error}</p>
				</details>
			{/if}
			<p class="flex items-center gap-1 text-xs text-muted-foreground">
				<TimeIcon class="size-3.5 shrink-0" aria-hidden="true" />
				{cert.failure_count}
				{cert.failure_count === 1 ? 'failed attempt' : 'failed attempts'}.
				{#if retryIn}
					Next automatic try in {retryIn}.
				{:else}
					Next automatic try within a minute.
				{/if}
			</p>
		</div>
	{:else if problem === 'expiring'}
		<p class="rounded-md border border-warn/40 bg-warn/10 p-3 text-sm">
			Tond renews certificates 30 days before they expire, so this one should have been renewed
			already. Check that tond-proxy is running (System page) and retry.
		</p>
	{/if}
</li>
