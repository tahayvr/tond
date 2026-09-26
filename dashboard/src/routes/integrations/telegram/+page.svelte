<script lang="ts">
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Send from '~icons/ri/send-plane-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, TelegramConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// ── Form fields ───────────────────────────────────────────────────────────
	// The bot token is write-only: a saved one is never shown or prefilled.
	let botToken = $state('');
	let tokenSaved = $state(false);
	let tokenReplacing = $state(false);
	let chatId = $state('');
	let threadId = $state('');

	function buildConfig(): IntegrationConfig {
		const token = secretFieldUpdate({
			saved: tokenSaved,
			replacing: tokenReplacing,
			removed: false,
			value: botToken
		});
		if (token === undefined && (!tokenSaved || tokenReplacing)) {
			throw new Error('Bot Token is required');
		}
		if (!chatId.trim()) throw new Error('Chat ID is required');
		return {
			...(token !== undefined && { bot_token: token }),
			chat_id: chatId.trim(),
			thread_id: threadId.trim() || undefined
		};
	}

	function resetConfig() {
		botToken = '';
		tokenSaved = false;
		tokenReplacing = false;
		chatId = '';
		threadId = '';
	}

	function loadConfig(config: IntegrationConfig, savedSecrets: string[]) {
		const c = config as TelegramConfig;
		botToken = '';
		tokenReplacing = false;
		tokenSaved = savedSecrets.includes('bot_token');
		chatId = c.chat_id;
		threadId = c.thread_id ?? '';
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
			<Send class="h-6 w-6" />
			<h2 class="text-3xl font-extrabold uppercase">Telegram</h2>
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
					<SavedSecretField
						id="tg-token"
						saved={tokenSaved}
						bind:value={botToken}
						bind:replacing={tokenReplacing}
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
