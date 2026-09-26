<script lang="ts">
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Webhook from '~icons/ri/webhook-line';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// ── Form fields ───────────────────────────────────────────────────────────
	// The webhook URL is write-only: a saved one is never shown or prefilled.
	let webhookUrl = $state('');
	let webhookSaved = $state(false);
	let webhookReplacing = $state(false);

	function buildConfig(): IntegrationConfig {
		const url = secretFieldUpdate({
			saved: webhookSaved,
			replacing: webhookReplacing,
			removed: false,
			value: webhookUrl
		});
		if (url === undefined && (!webhookSaved || webhookReplacing)) {
			throw new Error('Webhook URL is required');
		}
		return url === undefined ? {} : { webhook_url: url };
	}

	function resetConfig() {
		webhookUrl = '';
		webhookSaved = false;
		webhookReplacing = false;
	}

	function loadConfig(_config: IntegrationConfig, savedSecrets: string[]) {
		webhookUrl = '';
		webhookReplacing = false;
		webhookSaved = savedSecrets.includes('webhook_url');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-8">
	<div class="flex h-14 items-center gap-4">
		<a href="/integrations" class="text-muted-foreground hover:text-foreground">
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<Webhook class="h-6 w-6" />
			<h2 class="text-3xl font-extrabold uppercase">Discord</h2>
		</div>
	</div>

	<NotificationChannelList
		kind="discord"
		label="Discord"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="dc-url">Discord Webhook URL</Label>
				<SavedSecretField
					id="dc-url"
					kind="url"
					saved={webhookSaved}
					bind:value={webhookUrl}
					bind:replacing={webhookReplacing}
					placeholder="https://discord.com/api/webhooks/..."
				/>
				<p class="text-[10px] text-muted-foreground">
					The URL contains a token, so it is stored encrypted and never shown again.
				</p>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
