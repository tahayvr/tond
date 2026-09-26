<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import CodeIcon from '~icons/ri/code-s-slash-line';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, WebhookChannelConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// Both fields are write-only: saved values are never shown or prefilled.
	let url = $state('');
	let urlSaved = $state(false);
	let urlReplacing = $state(false);
	let secret = $state('');
	let secretSaved = $state(false);
	let secretReplacing = $state(false);
	let secretRemoved = $state(false);

	function buildConfig(): IntegrationConfig {
		const u = secretFieldUpdate({
			saved: urlSaved,
			replacing: urlReplacing,
			removed: false,
			value: url
		});
		if (u === undefined && (!urlSaved || urlReplacing)) {
			throw new Error('Enter the URL to POST alerts to.');
		}
		const s = secretFieldUpdate({
			saved: secretSaved,
			replacing: secretReplacing,
			removed: secretRemoved,
			value: secret
		});
		const config: WebhookChannelConfig = {};
		if (u !== undefined) config.webhook_url = u;
		// "" means "remove" for the form helper; the API clears with null.
		if (s === '') config.signing_secret = null;
		else if (s !== undefined) config.signing_secret = s;
		return config;
	}

	function resetConfig() {
		url = '';
		urlSaved = false;
		urlReplacing = false;
		secret = '';
		secretSaved = false;
		secretReplacing = false;
		secretRemoved = false;
	}

	function loadConfig(_config: IntegrationConfig, savedSecrets: string[]) {
		resetConfig();
		urlSaved = savedSecrets.includes('webhook_url');
		secretSaved = savedSecrets.includes('signing_secret');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});

	const EXAMPLE = `{
  "version": 1,
  "event": "firing",          // firing | repeat | resolved | test
  "delivery_id": "…",
  "sent_at": "2026-09-26T08:12:00+00:00",
  "alert": {
    "id": "…", "rule_id": "…", "rule_name": "High memory",
    "kind": "memory", "severity": "critical",
    "target": "app:…", "target_label": "api", "app_id": "…",
    "value": 93.2, "threshold": 90, "unit": "%", "comparison": "above",
    "summary": "Memory usage is 93.2% (threshold: above 90.0%).",
    "pending_since": "…", "fired_at": "…", "resolved_at": null,
    "duration_secs": 300, "url": "https://tond.example.com/alerts?alert=…"
  },
  "text": "Firing: High memory · api\\n…"
}`;
</script>

<PageTitle title="Webhook · Integrations" />

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
			<CodeIcon class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Webhook</h1>
		</div>
	</div>

	<p class="max-w-2xl text-sm text-muted-foreground">
		Tond POSTs every alert as JSON to your URL: when it fires, reminders, and when it resolves. Any
		2xx answer counts as delivered; anything else is retried for a few hours.
	</p>

	<NotificationChannelList
		kind="webhook"
		label="Webhook"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="wh-url">URL</Label>
				<SavedSecretField
					id="wh-url"
					kind="url"
					saved={urlSaved}
					bind:value={url}
					bind:replacing={urlReplacing}
					placeholder="https://hooks.example.com/tond"
				/>
			</div>
			<div class="grid gap-1.5">
				<Label class="text-xs" for="wh-secret">Signing secret (optional)</Label>
				<SavedSecretField
					id="wh-secret"
					saved={secretSaved}
					allowRemove
					bind:value={secret}
					bind:replacing={secretReplacing}
					bind:removed={secretRemoved}
					placeholder="a long random string"
				/>
				<p class="text-[10px] text-muted-foreground">
					With a secret, each request carries
					<code>X-Tond-Signature-256: sha256=&lt;HMAC-SHA256 of the body&gt;</code>, like GitHub
					webhooks.
				</p>
			</div>
		{/snippet}
	</NotificationChannelList>

	<section class="max-w-2xl space-y-2" aria-labelledby="payload-heading">
		<h2 id="payload-heading" class="text-sm font-semibold">Payload</h2>
		<p class="text-sm text-muted-foreground">
			Headers: <code>X-Tond-Event</code>, <code>X-Tond-Delivery</code> and, with a secret,
			<code>X-Tond-Signature-256</code>.
		</p>
		<pre class="overflow-x-auto rounded-sm border bg-muted/40 p-3 text-xs"><code>{EXAMPLE}</code
			></pre>
	</section>
</div>
