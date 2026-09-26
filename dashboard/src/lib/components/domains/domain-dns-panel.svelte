<!--
  DomainDnsPanel — the live DNS check for one domain: headline, what it
  resolves to, the exact records to create (the apps-domain wildcard when it
  applies) and "Check again". The parent owns the check and its polling.
-->
<script lang="ts" module>
	import type { DnsCheckStatus } from '$lib/api';

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
</script>

<script lang="ts">
	import type { DomainCheck } from '$lib/api';
	import { relativeTime } from '$lib/deploy-view';
	import { cn } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import OkIcon from '~icons/ri/checkbox-circle-fill';
	import WarnIcon from '~icons/ri/error-warning-line';
	import InfoIcon from '~icons/ri/information-line';
	import RefreshIcon from '~icons/ri/refresh-line';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';

	interface Props {
		check: DomainCheck | null;
		checking: boolean;
		checkError: string;
		now: number;
		/** The domain is the app's automatic address (the wildcard record covers it). */
		automatic: boolean;
		onCheckAgain: () => void;
	}

	let { check, checking, checkError, now, automatic, onCheckAgain }: Props = $props();

	let copied = $state('');

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
			<Button variant="outline" size="sm" onclick={onCheckAgain} disabled={checking}>
				<RefreshIcon class={cn(checking && 'animate-spin motion-reduce:animate-none')} />
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
				Tond couldn't determine this server's public IP address. Point the domain at the server's
				public IP with an A record (and AAAA for IPv6).
			</p>
		{:else if check.dns.status !== 'ok' || check.dns.stray.length > 0}
			{#if automatic && check.wildcard_records.length > 0}
				<p class="text-sm">
					Create this record at your DNS provider{check.zone ? ` for ${check.zone}` : ''}; it covers
					every app under <code>{check.apps_domain}</code>:
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
						automatic && check.wildcard_records.length ? check.wildcard_records : check.records,
						'DNS records'
					)}
				</div>
			</details>
		{/if}
		{#if check.dns_provider}
			<p class="text-xs text-muted-foreground">
				Certificates for this domain are validated through {check.dns_provider} (DNS-01), so they work
				even behind a CDN or before the A record exists.
			</p>
		{/if}
	{/if}
</section>
