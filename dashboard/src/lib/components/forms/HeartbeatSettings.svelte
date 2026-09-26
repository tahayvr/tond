<!--
  HeartbeatSettings — the outbound heartbeat (dead-man's switch): a URL at an
  outside monitor that Tond pings on a schedule while it is healthy. The URL
  is write-only: once saved only its host is shown.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import { getHeartbeat, testHeartbeat, updateHeartbeat, type HeartbeatStatus } from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { heartbeatLine } from '$lib/host-format';
	import { relativeUnix } from '$lib/uptime';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { SecretInput } from '$lib/components/ui/secret-input/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import PulseIcon from '~icons/ri/pulse-line';

	interface Props {
		canEdit: boolean;
	}

	let { canEdit }: Props = $props();

	const INTERVALS = [60, 120, 300, 600, 900, 1800, 3600];

	let status = $state<HeartbeatStatus | null>(null);
	let loadError = $state('');
	let url = $state('');
	let method = $state<'GET' | 'POST'>('GET');
	let interval = $state('300');
	let enabled = $state(true);
	let saved = $state('');
	let formError = $state('');
	let initialized = false;

	function fill(s: HeartbeatStatus) {
		status = s;
		method = s.method;
		interval = String(s.interval_secs);
		enabled = s.enabled;
	}

	$effect(() => {
		untrack(async () => {
			try {
				const s = await getHeartbeat();
				if (!initialized) {
					initialized = true;
					fill(s);
				}
			} catch (e: any) {
				loadError = e.message ?? 'request failed';
			}
		});
	});

	function intervalLabel(secs: number): string {
		const min = secs / 60;
		return min >= 60 ? 'Every hour' : min === 1 ? 'Every minute' : `Every ${min} minutes`;
	}

	const save = useAction(
		() =>
			updateHeartbeat({
				...(url.trim() ? { url: url.trim() } : {}),
				method,
				interval_secs: Number(interval),
				enabled
			}),
		{
			onSuccess: (s) => {
				fill(s);
				url = '';
				saved = s.configured
					? `Saved. ${s.enabled ? `Next ping ${relativeUnix(s.next_at, Date.now() / 1000)}.` : 'Heartbeats are off.'}`
					: 'Saved.';
			}
		}
	);

	const test = useAction(() => testHeartbeat(), {
		onSuccess: (r) => {
			status = r.heartbeat;
			if (r.outcome === 'sent') saved = `Test ping sent: your monitor answered HTTP ${r.status}.`;
		}
	});

	function handleSave(e: Event) {
		e.preventDefault();
		saved = '';
		formError = '';
		const value = url.trim();
		if (!value && !status?.configured) {
			formError = 'Paste the ping URL your monitor gave you (it starts with https://).';
			return;
		}
		if (value && !/^https?:\/\/[^\s/]+/i.test(value)) {
			formError = 'Enter the full URL, starting with https://';
			return;
		}
		save.run();
	}

	async function handleTest() {
		saved = '';
		const r = await test.run();
		if (r && r.outcome === 'failed') formError = `The test ping failed: ${r.error}`;
	}

	async function handleRemove() {
		await confirmAction({
			title: 'Remove the heartbeat?',
			description:
				'Tond stops pinging your monitor, so it will alert that the server is down unless you pause or delete the check there first.',
			confirmLabel: 'Remove heartbeat',
			onconfirm: async () => {
				const s = await updateHeartbeat({ url: '' });
				fill(s);
				saved = 'Heartbeat removed.';
			}
		});
	}

	let line = $derived(status ? heartbeatLine(status) : null);
</script>

<section id="heartbeat" class="scroll-mt-20 rounded-lg border border-border bg-card p-4">
	<div class="mb-1 flex flex-wrap items-center justify-between gap-2">
		<div class="flex items-center gap-2">
			<PulseIcon class="size-4 text-muted-foreground" aria-hidden="true" />
			<h2 class="font-semibold">Heartbeat (dead-man’s switch)</h2>
		</div>
		{#if line}
			<StatusBadge
				status={line.tone === 'success'
					? 'healthy'
					: line.tone === 'warning'
						? 'degraded'
						: line.tone === 'danger'
							? 'failed'
							: 'disabled'}
				label={line.tone === 'success'
					? 'Sending'
					: line.tone === 'warning'
						? 'Holding pings'
						: line.tone === 'danger'
							? 'Failing'
							: status?.configured
								? 'Off'
								: 'Not set up'}
				size="sm"
			/>
		{/if}
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		A server can't report its own outage. Tond pings an outside monitor on a schedule while it's
		healthy (Docker answers, tond-proxy is connected and Tond's background work is running). When
		the pings stop — the server is down, or Tond is — the monitor alerts you.
	</p>

	<InlineError message={loadError ? `Couldn't load the heartbeat settings: ${loadError}` : ''} />

	{#if !status && !loadError}
		<Skeleton class="h-32" />
	{:else if status}
		{#if line && status.configured}
			<p class="mb-4 text-sm" data-testid="heartbeat-status">
				{line.text}.
				{#if status.last_failure_at && status.last_error && (status.last_failure_at ?? 0) > (status.last_ok_at ?? 0)}
					<span class="text-muted-foreground">
						Check the URL and that the monitor is reachable from this server.</span
					>
				{/if}
				{#if !status.healthy && status.unhealthy_reason}
					<span class="block text-warn">
						Right now Tond would hold the ping: {status.unhealthy_reason}.
					</span>
				{/if}
			</p>
		{/if}

		<form class="space-y-4" onsubmit={handleSave} novalidate>
			<div class="grid max-w-2xl gap-1.5">
				<Label for="heartbeat-url">Ping URL</Label>
				<SecretInput
					id="heartbeat-url"
					bind:value={url}
					copyable={false}
					autocomplete="off"
					spellcheck={false}
					placeholder={status.configured
						? `Saved (${status.url_host}). Paste a new URL to replace it.`
						: 'https://hc-ping.com/…'}
					disabled={!canEdit}
				/>
				<p class="text-xs text-muted-foreground">
					Stored encrypted and never shown again: anyone with it could report your server healthy.
				</p>
			</div>
			<div class="grid max-w-2xl gap-4 sm:grid-cols-2">
				<div class="grid gap-1.5">
					<Label for="heartbeat-interval">Interval</Label>
					<Select.Root type="single" bind:value={interval} disabled={!canEdit}>
						<Select.Trigger id="heartbeat-interval" class="w-full"
							>{intervalLabel(Number(interval))}</Select.Trigger
						>
						<Select.Content>
							{#each INTERVALS as secs (secs)}
								<Select.Item value={String(secs)} label={intervalLabel(secs)} />
							{/each}
						</Select.Content>
					</Select.Root>
				</div>
				<div class="grid gap-1.5">
					<Label for="heartbeat-method">Request</Label>
					<Select.Root
						type="single"
						value={method}
						onValueChange={(v) => (method = v === 'POST' ? 'POST' : 'GET')}
						disabled={!canEdit}
					>
						<Select.Trigger id="heartbeat-method" class="w-full">{method}</Select.Trigger>
						<Select.Content>
							<Select.Item value="GET" label="GET" />
							<Select.Item value="POST" label="POST" />
						</Select.Content>
					</Select.Root>
				</div>
			</div>
			<div class="flex items-center gap-2">
				<Checkbox id="heartbeat-enabled" bind:checked={enabled} disabled={!canEdit} />
				<Label for="heartbeat-enabled">Send heartbeats</Label>
			</div>

			<InlineError
				message={formError || save.error || test.error}
				ondismiss={() => ((formError = ''), save.clear(), test.clear())}
			/>
			<SuccessAlert message={saved} timeout={6000} ondismiss={() => (saved = '')} />

			{#if canEdit}
				<div class="flex flex-wrap gap-2">
					<Button type="submit" size="sm" disabled={save.pending}>
						{save.pending ? 'Saving…' : 'Save'}
					</Button>
					{#if status.configured}
						<Button
							type="button"
							size="sm"
							variant="outline"
							onclick={handleTest}
							disabled={test.pending}
						>
							{test.pending ? 'Sending…' : 'Send test ping'}
						</Button>
						<Button type="button" size="sm" variant="ghost" onclick={handleRemove}>Remove</Button>
					{/if}
				</div>
			{:else}
				<p class="text-xs text-muted-foreground">Only admins can change this.</p>
			{/if}
		</form>

		<details class="mt-5 max-w-2xl text-sm">
			<summary class="cursor-pointer font-medium">Where do I get a ping URL?</summary>
			<ul class="mt-3 space-y-3 text-muted-foreground">
				<li>
					<span class="font-medium text-foreground">Healthchecks.io</span> — add a check, set its
					period to the interval above and a grace time of a few minutes, then copy its ping URL (<code
						class="font-mono text-xs">https://hc-ping.com/…</code
					>).
				</li>
				<li>
					<span class="font-medium text-foreground">Uptime Kuma</span> — add a monitor of type
					<em>Push</em> with the same heartbeat interval and copy the push URL (<code
						class="font-mono text-xs">…/api/push/…?status=up</code
					>).
				</li>
				<li>
					<span class="font-medium text-foreground">Better Stack</span> — create a heartbeat with
					the same period and copy its URL (<code class="font-mono text-xs"
						>https://uptime.betterstack.com/api/v1/heartbeat/…</code
					>).
				</li>
				<li>
					Use a monitor that runs somewhere else: a check on this server stops with it. Allow a
					grace period of about twice the interval so one slow ping doesn't page you.
				</li>
			</ul>
		</details>
	{/if}
</section>
