<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import NtfyIcon from '~icons/ri/notification-badge-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, NtfyConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// The topic (on a public server, anyone who knows it can read it) and the
	// token are write-only: saved values are never shown or prefilled.
	let serverUrl = $state('https://ntfy.sh');
	let topic = $state('');
	let topicSaved = $state(false);
	let topicReplacing = $state(false);
	let token = $state('');
	let tokenSaved = $state(false);
	let tokenReplacing = $state(false);
	let tokenRemoved = $state(false);

	function buildConfig(): IntegrationConfig {
		if (!serverUrl.trim())
			throw new Error('Enter the ntfy server URL (https://ntfy.sh for the public one).');
		const t = secretFieldUpdate({
			saved: topicSaved,
			replacing: topicReplacing,
			removed: false,
			value: topic
		});
		if (t === undefined && (!topicSaved || topicReplacing)) {
			throw new Error('Enter a topic.');
		}
		const tok = secretFieldUpdate({
			saved: tokenSaved,
			replacing: tokenReplacing,
			removed: tokenRemoved,
			value: token
		});
		const config: NtfyConfig = { server_url: serverUrl.trim() };
		if (t !== undefined) config.topic = t;
		// "" means "remove" for the form helper; the API clears with null.
		if (tok === '') config.token = null;
		else if (tok !== undefined) config.token = tok;
		return config;
	}

	function resetConfig() {
		serverUrl = 'https://ntfy.sh';
		topic = '';
		topicSaved = false;
		topicReplacing = false;
		token = '';
		tokenSaved = false;
		tokenReplacing = false;
		tokenRemoved = false;
	}

	function loadConfig(config: IntegrationConfig, savedSecrets: string[]) {
		const c = config as NtfyConfig;
		serverUrl = c.server_url ?? 'https://ntfy.sh';
		topic = '';
		topicReplacing = false;
		topicSaved = savedSecrets.includes('topic');
		token = '';
		tokenReplacing = false;
		tokenRemoved = false;
		tokenSaved = savedSecrets.includes('token');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="ntfy · Integrations" />

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
			<NtfyIcon class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">ntfy</h1>
		</div>
	</div>

	<p class="max-w-2xl text-sm text-muted-foreground">
		Push notifications to your phone through <a
			href="https://ntfy.sh"
			class="underline underline-offset-2"
			rel="noreferrer"
			target="_blank">ntfy</a
		>, the public server or your own. Critical alerts arrive with urgent priority.
	</p>

	<NotificationChannelList
		kind="ntfy"
		label="ntfy"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="ntfy-server">Server URL</Label>
				<Input
					id="ntfy-server"
					class="h-8 text-xs"
					type="url"
					bind:value={serverUrl}
					placeholder="https://ntfy.sh"
				/>
			</div>
			<div class="grid gap-1.5">
				<Label class="text-xs" for="ntfy-topic">Topic</Label>
				<SavedSecretField
					id="ntfy-topic"
					saved={topicSaved}
					bind:value={topic}
					bind:replacing={topicReplacing}
					placeholder="tond-alerts-7f3k9q"
				/>
				<p class="text-[10px] text-muted-foreground">
					On ntfy.sh anyone who knows the topic can read it: pick a long, random name. It is stored
					encrypted.
				</p>
			</div>
			<div class="grid gap-1.5">
				<Label class="text-xs" for="ntfy-token">Access token (optional)</Label>
				<SavedSecretField
					id="ntfy-token"
					saved={tokenSaved}
					allowRemove
					bind:value={token}
					bind:replacing={tokenReplacing}
					bind:removed={tokenRemoved}
					placeholder="tk_…"
				/>
				<p class="text-[10px] text-muted-foreground">For topics that need a login.</p>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
