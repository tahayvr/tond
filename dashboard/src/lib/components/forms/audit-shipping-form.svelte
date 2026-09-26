<!--
  AuditShippingForm — copy every activity-log entry (with its chain hash) to a
  syslog server and/or a signed HTTPS webhook. Admin only; loads its own data.
-->
<script lang="ts">
	import {
		getAuditShipping,
		testAuditShipping,
		updateAuditShipping,
		type AuditShipping,
		type ShippingDestination
	} from '$lib/api';
	import { generateSecret, shippingSummary } from '$lib/audit-shipping';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import { cn } from '$lib/utils';
	import ShipIcon from '~icons/ri/ship-2-line';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import ErrorIcon from '~icons/ri/error-warning-line';

	let data = $state<AuditShipping | null>(null);
	let loadError = $state('');
	let now = $state(Date.now());

	// Syslog form
	let syslogEnabled = $state(false);
	let syslogHost = $state('');
	let syslogPort = $state<number | null>(6514);
	let syslogTls = $state(true);
	// Webhook form
	let webhookEnabled = $state(false);
	let webhookUrl = $state('');
	let webhookSecret = $state('');
	let secretShown = $state(false);

	let saving = $state<ShippingDestination | null>(null);
	let testing = $state<ShippingDestination | null>(null);
	let errors = $state<Record<ShippingDestination, string>>({ syslog: '', webhook: '' });
	let results = $state<Record<ShippingDestination, { ok: boolean; message: string } | null>>({
		syslog: null,
		webhook: null
	});

	let syslogSummary = $derived(
		data ? shippingSummary(data.syslog.enabled, data.syslog.status, now) : null
	);
	let webhookSummary = $derived(
		data ? shippingSummary(data.webhook.enabled, data.webhook.status, now) : null
	);

	function apply(next: AuditShipping) {
		data = next;
		now = Date.now();
		syslogEnabled = next.syslog.enabled;
		syslogHost = next.syslog.host;
		syslogPort = next.syslog.port;
		syslogTls = next.syslog.tls;
		webhookEnabled = next.webhook.enabled;
		webhookUrl = next.webhook.url;
		webhookSecret = '';
		secretShown = false;
	}

	async function load() {
		try {
			apply(await getAuditShipping());
			loadError = '';
		} catch (e: any) {
			loadError = e.message;
		}
	}

	$effect(() => {
		load();
	});

	async function handleSave(destination: ShippingDestination, e: Event) {
		e.preventDefault();
		errors[destination] = '';
		results[destination] = null;
		if (destination === 'syslog') {
			const port = syslogPort ?? 0;
			if (!Number.isInteger(port) || port < 1 || port > 65535) {
				errors.syslog = 'The port must be between 1 and 65535.';
				return;
			}
		}
		saving = destination;
		try {
			const next = await updateAuditShipping(
				destination === 'syslog'
					? {
							syslog: {
								enabled: syslogEnabled,
								host: syslogHost.trim(),
								port: syslogPort ?? 6514,
								tls: syslogTls
							}
						}
					: {
							webhook: {
								enabled: webhookEnabled,
								url: webhookUrl.trim(),
								secret: webhookSecret.trim() || null
							}
						}
			);
			apply(next);
			results[destination] = { ok: true, message: 'Saved.' };
		} catch (err: any) {
			errors[destination] = err.message;
		} finally {
			saving = null;
		}
	}

	async function handleTest(destination: ShippingDestination) {
		errors[destination] = '';
		results[destination] = null;
		testing = destination;
		try {
			results[destination] = await testAuditShipping(destination);
		} catch (err: any) {
			errors[destination] = err.message;
		} finally {
			testing = null;
		}
	}

	function handleGenerate() {
		webhookSecret = generateSecret();
		secretShown = true;
	}
</script>

{#snippet status(summary: ReturnType<typeof shippingSummary> | null)}
	{#if summary}
		<div class="flex items-start gap-2 text-xs">
			<StatusBadge status="custom" tone={summary.tone} label={summary.label} size="sm" />
			<p class="min-w-0 pt-0.5 break-words text-muted-foreground">{summary.detail}</p>
		</div>
	{/if}
{/snippet}

{#snippet result(destination: ShippingDestination)}
	{@const r = results[destination]}
	{#if r}
		<p
			class={cn('flex items-start gap-1.5 text-xs', r.ok ? 'text-success' : 'text-destructive')}
			role="status"
		>
			{#if r.ok}<CheckIcon class="mt-0.5 size-3.5 shrink-0" />{:else}<ErrorIcon
					class="mt-0.5 size-3.5 shrink-0"
				/>{/if}
			<span class="break-words">{r.message}</span>
		</p>
	{/if}
	<InlineError message={errors[destination]} ondismiss={() => (errors[destination] = '')} />
{/snippet}

<section class="rounded-lg border border-border bg-card p-4" aria-labelledby="shipping-title">
	<div class="mb-1 flex items-center gap-2">
		<ShipIcon class="size-4 text-muted-foreground" aria-hidden="true" />
		<h2 id="shipping-title" class="font-semibold">Audit log shipping</h2>
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		Copy every activity log entry, with its chain hash, to a system this server can't rewrite. If
		someone with root access edits or cuts off the local log, the copy shows what it should say. Off
		by default.
	</p>

	<InlineError message={loadError} />

	{#if !data && !loadError}
		<div class="grid gap-4 lg:grid-cols-2">
			<Skeleton class="h-56" />
			<Skeleton class="h-56" />
		</div>
	{:else if data}
		<div class="grid gap-4 lg:grid-cols-2">
			<form
				class="space-y-3 rounded-md border border-border p-3"
				onsubmit={(e) => handleSave('syslog', e)}
				novalidate
			>
				<div class="flex items-center gap-2">
					<Checkbox id="syslog-enabled" bind:checked={syslogEnabled} />
					<Label for="syslog-enabled" class="font-medium">Syslog server</Label>
				</div>
				<p class="text-xs text-muted-foreground">
					RFC 5424 messages over TCP (RFC 6587 framing), normally with TLS. Works with rsyslog,
					syslog-ng, Vector, Graylog and most log services.
				</p>
				<div class="grid grid-cols-[minmax(0,1fr)_6rem] gap-2">
					<div class="grid gap-1">
						<Label for="syslog-host" class="text-xs">Host</Label>
						<Input
							id="syslog-host"
							placeholder="logs.example.com"
							autocomplete="off"
							bind:value={syslogHost}
						/>
					</div>
					<div class="grid gap-1">
						<Label for="syslog-port" class="text-xs">Port</Label>
						<Input id="syslog-port" type="number" min={1} max={65535} bind:value={syslogPort} />
					</div>
				</div>
				<div class="flex items-center gap-2">
					<Checkbox id="syslog-tls" bind:checked={syslogTls} />
					<Label for="syslog-tls" class="text-sm font-normal"
						>Use TLS (certificate checked against the system's trusted CAs)</Label
					>
				</div>
				{#if !syslogTls}
					<p class="text-xs text-warn">
						Without TLS, entries cross the network in plain text and can be altered in transit.
					</p>
				{/if}
				{@render status(syslogSummary)}
				{@render result('syslog')}
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" disabled={saving !== null}>
						{saving === 'syslog' ? 'Saving…' : 'Save'}
					</Button>
					<Button
						type="button"
						variant="outline"
						size="sm"
						onclick={() => handleTest('syslog')}
						disabled={testing !== null || !data.syslog.host}
						title={data.syslog.host ? undefined : 'Save a host first'}
					>
						{testing === 'syslog' ? 'Sending…' : 'Send test'}
					</Button>
				</div>
			</form>

			<form
				class="space-y-3 rounded-md border border-border p-3"
				onsubmit={(e) => handleSave('webhook', e)}
				novalidate
			>
				<div class="flex items-center gap-2">
					<Checkbox id="webhook-enabled" bind:checked={webhookEnabled} />
					<Label for="webhook-enabled" class="font-medium">HTTPS webhook</Label>
				</div>
				<p class="text-xs text-muted-foreground">
					POSTs batches of entries as JSON, signed with HMAC-SHA256. Failed deliveries are retried
					with backoff; nothing is lost across restarts.
				</p>
				<div class="grid gap-1">
					<Label for="webhook-url" class="text-xs">URL</Label>
					<Input
						id="webhook-url"
						type="url"
						placeholder="https://logs.example.com/tond"
						autocomplete="off"
						bind:value={webhookUrl}
					/>
				</div>
				<div class="grid gap-1">
					<Label for="webhook-secret" class="text-xs">Signing secret</Label>
					<div class="flex gap-2">
						<Input
							id="webhook-secret"
							type={secretShown ? 'text' : 'password'}
							class="font-mono"
							placeholder={data.webhook.has_secret
								? 'Stored — leave empty to keep it'
								: 'At least 16 characters'}
							autocomplete="new-password"
							bind:value={webhookSecret}
						/>
						<Button type="button" variant="outline" size="sm" class="h-9" onclick={handleGenerate}
							>Generate</Button
						>
					</div>
					{#if secretShown && webhookSecret}
						<p class="text-xs text-muted-foreground">
							Copy it to the receiver now: after saving it is never shown again.
						</p>
					{/if}
				</div>
				{@render status(webhookSummary)}
				{@render result('webhook')}
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" disabled={saving !== null}>
						{saving === 'webhook' ? 'Saving…' : 'Save'}
					</Button>
					<Button
						type="button"
						variant="outline"
						size="sm"
						onclick={() => handleTest('webhook')}
						disabled={testing !== null || !data.webhook.url}
						title={data.webhook.url ? undefined : 'Save a URL first'}
					>
						{testing === 'webhook' ? 'Sending…' : 'Send test'}
					</Button>
				</div>
			</form>
		</div>

		<details class="mt-4 rounded-md border border-border p-3 text-sm">
			<summary class="cursor-pointer font-medium select-none">How to check the copy</summary>
			<div class="mt-3 space-y-3 text-muted-foreground">
				<p>
					Each entry carries <code class="font-mono text-xs">seq</code>,
					<code class="font-mono text-xs">prev_hash</code> and
					<code class="font-mono text-xs">hash</code>. In seq order,
					<code class="font-mono text-xs">prev_hash</code> must equal the previous entry's hash, and
				</p>
				<pre
					class="overflow-x-auto rounded bg-muted p-2 font-mono text-xs text-foreground">hash = sha256(prev_hash + json([seq, created_at, actor_kind, actor_id,
       actor_label, action, resource_type, resource_id, resource_name,
       outcome, ip, detail]))</pre>
				<p>
					(compact JSON, no spaces). Then compare with this server: run
					<code class="font-mono text-xs">tond audit verify</code> or "Verify integrity" on the
					Activity page, and check that its newest entry (#{data.latest_seq}) has the same hash as
					in the copy. An entry in the copy that is missing here, or a different hash for the same
					seq, means the local log was rewritten.
				</p>
				<p>
					Webhook receivers should check
					<code class="font-mono text-xs">X-Tond-Signature</code>: it is
					<code class="font-mono text-xs">sha256=</code> followed by the hex HMAC-SHA256 of
					<code class="font-mono text-xs">X-Tond-Timestamp + "." + body</code> with the signing secret.
					Reject old timestamps to stop replays.
				</p>
			</div>
		</details>
	{/if}
</section>
