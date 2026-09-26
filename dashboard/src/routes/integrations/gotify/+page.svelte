<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import GotifyIcon from '~icons/ri/notification-2-line';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { GotifyConfig, Integration, IntegrationConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	let serverUrl = $state('');
	// The app token is write-only: a saved one is never shown or prefilled.
	let appToken = $state('');
	let tokenSaved = $state(false);
	let tokenReplacing = $state(false);

	function buildConfig(): IntegrationConfig {
		if (!serverUrl.trim()) throw new Error('Enter your Gotify server URL.');
		const token = secretFieldUpdate({
			saved: tokenSaved,
			replacing: tokenReplacing,
			removed: false,
			value: appToken
		});
		if (token === undefined && (!tokenSaved || tokenReplacing)) {
			throw new Error('Enter the application token.');
		}
		const config: GotifyConfig = { server_url: serverUrl.trim() };
		if (token !== undefined) config.app_token = token;
		return config;
	}

	function resetConfig() {
		serverUrl = '';
		appToken = '';
		tokenSaved = false;
		tokenReplacing = false;
	}

	function loadConfig(config: IntegrationConfig, savedSecrets: string[]) {
		const c = config as GotifyConfig;
		serverUrl = c.server_url ?? '';
		appToken = '';
		tokenReplacing = false;
		tokenSaved = savedSecrets.includes('app_token');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Gotify · Integrations" />

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
			<GotifyIcon class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Gotify</h1>
		</div>
	</div>

	<p class="max-w-2xl text-sm text-muted-foreground">
		Push notifications to your self-hosted Gotify server. Create an application in Gotify for Tond
		and paste its token.
	</p>

	<NotificationChannelList
		kind="gotify"
		label="Gotify"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="gotify-server">Server URL</Label>
				<Input
					id="gotify-server"
					class="h-8 text-xs"
					type="url"
					bind:value={serverUrl}
					placeholder="https://gotify.example.com"
				/>
			</div>
			<div class="grid gap-1.5">
				<Label class="text-xs" for="gotify-token">Application token</Label>
				<SavedSecretField
					id="gotify-token"
					saved={tokenSaved}
					bind:value={appToken}
					bind:replacing={tokenReplacing}
					placeholder="AbCdEf123456"
				/>
				<p class="text-[10px] text-muted-foreground">Stored encrypted and never shown again.</p>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
