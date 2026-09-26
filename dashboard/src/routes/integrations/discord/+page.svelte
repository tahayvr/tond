<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Webhook from '~icons/ri/webhook-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, DiscordConfig } from '$lib/api';
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
		webhookUrl = (config as DiscordConfig).webhook_url;
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Discord · Integrations" />

<div class="space-y-8">
	<div class="flex h-14 items-center gap-4">
		<a href="/integrations" class="text-muted-foreground hover:text-foreground">
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<Webhook class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Discord</h1>
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
				<Input
					id="dc-url"
					class="h-8 text-xs"
					bind:value={webhookUrl}
					placeholder="https://discord.com/api/webhooks/..."
				/>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
