<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import PhoneIcon from '~icons/ri/smartphone-line';
	import { Label } from '$lib/components/ui/label/index.js';
	import SavedSecretField, {
		secretFieldUpdate
	} from '$lib/components/secrets/saved-secret-field.svelte';
	import NotificationChannelList from '$lib/components/integrations/notification-channel-list.svelte';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { Integration, IntegrationConfig, PushoverConfig } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let integrations = $derived((data as { integrations: Integration[] }).integrations);

	// Both keys are write-only: saved values are never shown or prefilled.
	let userKey = $state('');
	let userSaved = $state(false);
	let userReplacing = $state(false);
	let appToken = $state('');
	let appSaved = $state(false);
	let appReplacing = $state(false);

	function buildConfig(): IntegrationConfig {
		const user = secretFieldUpdate({
			saved: userSaved,
			replacing: userReplacing,
			removed: false,
			value: userKey
		});
		if (user === undefined && (!userSaved || userReplacing)) {
			throw new Error('Enter your Pushover user key.');
		}
		const app = secretFieldUpdate({
			saved: appSaved,
			replacing: appReplacing,
			removed: false,
			value: appToken
		});
		if (app === undefined && (!appSaved || appReplacing)) {
			throw new Error('Enter the application API token.');
		}
		const config: PushoverConfig = {};
		if (user !== undefined) config.user_key = user;
		if (app !== undefined) config.app_token = app;
		return config;
	}

	function resetConfig() {
		userKey = '';
		userSaved = false;
		userReplacing = false;
		appToken = '';
		appSaved = false;
		appReplacing = false;
	}

	function loadConfig(_config: IntegrationConfig, savedSecrets: string[]) {
		resetConfig();
		userSaved = savedSecrets.includes('user_key');
		appSaved = savedSecrets.includes('app_token');
	}

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});
</script>

<PageTitle title="Pushover · Integrations" />

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
			<PhoneIcon class="h-6 w-6" />
			<h1 class="text-2xl font-extrabold sm:text-3xl">Pushover</h1>
		</div>
	</div>

	<p class="max-w-2xl text-sm text-muted-foreground">
		Push notifications through <a
			href="https://pushover.net"
			class="underline underline-offset-2"
			rel="noreferrer"
			target="_blank">Pushover</a
		>. Create an application there for Tond and paste its API token with your user key. Critical
		alerts are sent with high priority.
	</p>

	<NotificationChannelList
		kind="pushover"
		label="Pushover"
		initialIntegrations={integrations}
		{buildConfig}
		{resetConfig}
		{loadConfig}
	>
		{#snippet configFields()}
			<div class="grid gap-1.5">
				<Label class="text-xs" for="po-user">User key</Label>
				<SavedSecretField
					id="po-user"
					saved={userSaved}
					bind:value={userKey}
					bind:replacing={userReplacing}
					placeholder="uQiRzpo4DXghDmr9QzzfQu27cmVRsG"
				/>
			</div>
			<div class="grid gap-1.5">
				<Label class="text-xs" for="po-app">Application API token</Label>
				<SavedSecretField
					id="po-app"
					saved={appSaved}
					bind:value={appToken}
					bind:replacing={appReplacing}
					placeholder="azGDORePK8gMaC0QOYAMyEEuzJnyUi"
				/>
				<p class="text-[10px] text-muted-foreground">
					Both are stored encrypted and never shown again.
				</p>
			</div>
		{/snippet}
	</NotificationChannelList>
</div>
