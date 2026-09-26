<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Send from '~icons/ri/send-plane-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, TelegramConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// ── Form fields ───────────────────────────────────────────────────────────
	let botToken = $state('');
	let chatId = $state('');
	let threadId = $state('');

	function buildConfig(): IntegrationConfig {
		if (!botToken.trim()) throw new Error('Bot Token is required');
		if (!chatId.trim()) throw new Error('Chat ID is required');
		return {
			bot_token: botToken.trim(),
			chat_id: chatId.trim(),
			thread_id: threadId.trim() || undefined
		};
	}

	function resetConfig() {
		botToken = '';
		chatId = '';
		threadId = '';
	}

	function loadConfig(config: IntegrationConfig) {
		const c = config as TelegramConfig;
		botToken = c.bot_token;
		chatId = c.chat_id;
		threadId = c.thread_id ?? '';
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title={'Telegram · Integrations'} />

<div class="space-y-8">
	<div class="flex h-14 items-center gap-4">
		<a href="/integrations" class="text-muted-foreground hover:text-foreground">
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<Send class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Telegram</h1>
		</div>
	</div>

	<NotificationChannelList
		kind="telegram"
		label="Telegram"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="grid gap-1.5">
					<Label class="text-xs" for="tg-token">Bot Token</Label>
					<Input
						id="tg-token"
						class="h-8 text-xs"
						bind:value={botToken}
						placeholder="123456:ABCdef..."
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="tg-chat">Chat ID</Label>
					<Input
						id="tg-chat"
						class="h-8 text-xs"
						bind:value={chatId}
						placeholder="-1001234567890"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="tg-thread"
						>Thread / Topic ID <span class="text-muted-foreground">(optional)</span></Label
					>
					<Input
						id="tg-thread"
						class="h-8 text-xs"
						bind:value={threadId}
						placeholder="Leave blank for main chat"
					/>
				</div>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
