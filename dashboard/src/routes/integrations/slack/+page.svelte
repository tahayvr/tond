<script lang="ts">
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import Webhook from '@lucide/svelte/icons/webhook';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, SlackConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// ── Form fields ───────────────────────────────────────────────────────────
	let webhookUrl = $state('');

	function buildConfig(): IntegrationConfig {
		if (!webhookUrl.trim()) throw new Error('Webhook URL is required');
		return { webhook_url: webhookUrl.trim() };
	}

	function resetConfig() {
		webhookUrl = '';
	}

	function loadConfig(config: IntegrationConfig) {
		webhookUrl = (config as SlackConfig).webhook_url;
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
			<h2 class="text-3xl font-extrabold uppercase">Slack / Mattermost</h2>
		</div>
	</div>

	<NotificationChannelList
		kind="slack"
		label="Slack / Mattermost"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="sl-url">Incoming Webhook URL</Label>
				<Input
					id="sl-url"
					class="h-8 text-xs"
					bind:value={webhookUrl}
					placeholder="https://hooks.slack.com/services/..."
				/>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
