<script lang="ts">
	import {
		type GitHubApp,
		type RegistryCredential,
		type Integration,
		type IntegrationKind
	} from '$lib/api';
	import IntegrationCard from '$lib/components/cards/integrations/integration-card.svelte';
	import StorageProviderList from '$lib/components/integrations/storage-provider-list.svelte';
	import Github from '@lucide/svelte/icons/github';
	import Package from '@lucide/svelte/icons/package';
	import Send from '@lucide/svelte/icons/send';
	import Webhook from '@lucide/svelte/icons/webhook';
	import Mail from '@lucide/svelte/icons/mail';
	import { setUiContext, clearUiContext, integrationsContext } from '$lib/ui-context.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	// ── GitHub ────────────────────────────────────────────────────────────────
	let githubApps = $derived(data.githubApps as GitHubApp[]);

	// ── Registry ─────────────────────────────────────────────────────────────
	let credentials = $derived(data.credentials as RegistryCredential[]);

	// ── Notification integrations ─────────────────────────────────────────────
	let integrations = $derived(data.integrations as Integration[]);

	$effect(() => {
		setUiContext(integrationsContext());
		return () => clearUiContext();
	});

	function githubStatus() {
		return githubApps.length > 0 ? ('connected' as const) : ('disconnected' as const);
	}

	function githubDetails() {
		if (githubApps.length === 0) return [];
		const app = githubApps[0];
		return [
			{ label: 'App', value: app.name },
			{
				label: 'Installation',
				value: app.installation_id != null ? String(app.installation_id) : 'Pending'
			}
		];
	}

	function registryStatus() {
		return credentials.length > 0 ? ('connected' as const) : ('disconnected' as const);
	}

	function registryDetails() {
		if (credentials.length === 0) return [];
		return [{ label: 'Credentials', value: String(credentials.length) }];
	}

	function channelStatus(kind: IntegrationKind) {
		const count = integrations.filter((i) => i.kind === kind && i.enabled).length;
		return count > 0 ? ('connected' as const) : ('disconnected' as const);
	}

	function channelDetails(kind: IntegrationKind) {
		const items = integrations.filter((i) => i.kind === kind);
		if (items.length === 0) return [];
		return [{ label: 'Configured', value: String(items.length) }];
	}
</script>

<div class="space-y-10">
	<div class="flex h-14 items-center justify-between">
		<h2 class="text-3xl font-extrabold uppercase">Integrations</h2>
	</div>

	<!-- ── Notification Channels ─────────────────────────────────────────────── -->
	<section class="space-y-4">
		<h3 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
			Notification Channels
		</h3>
		<div class="grid grid-cols-1 items-stretch gap-4 sm:grid-cols-2 lg:grid-cols-4">
			<IntegrationCard
				name="Telegram"
				icon={Send}
				status={channelStatus('telegram')}
				details={channelDetails('telegram')}
				href="/integrations/telegram"
			/>
			<IntegrationCard
				name="Discord"
				icon={Webhook}
				status={channelStatus('discord')}
				details={channelDetails('discord')}
				href="/integrations/discord"
			/>
			<IntegrationCard
				name="Slack / Mattermost"
				icon={Webhook}
				status={channelStatus('slack')}
				details={channelDetails('slack')}
				href="/integrations/slack"
			/>
			<IntegrationCard
				name="Email (SMTP)"
				icon={Mail}
				status={channelStatus('email')}
				details={channelDetails('email')}
				href="/integrations/email"
			/>
		</div>
	</section>

	<!-- ── Storage Providers ──────────────────────────────────────────────────── -->
	<section class="space-y-4">
		<h3 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
			Storage Providers
		</h3>
		<StorageProviderList />
	</section>

	<!-- ── Git Providers ─────────────────────────────────────────────────── -->
	<section class="space-y-4">
		<h3 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
			Git Providers
		</h3>
		<div class="grid grid-cols-1 items-stretch gap-4 sm:grid-cols-2 lg:grid-cols-3">
			<IntegrationCard
				name="GitHub"
				icon={Github}
				status={githubStatus()}
				details={githubDetails()}
				href="/integrations/github"
			/>
		</div>
	</section>

	<!-- ── Container Registry ─────────────────────────────────────────────── -->
	<section class="space-y-4">
		<h3 class="text-xs font-semibold tracking-widest text-muted-foreground uppercase">
			Container Registry
		</h3>
		<div class="grid grid-cols-1 items-stretch gap-4 sm:grid-cols-2 lg:grid-cols-3">
			<IntegrationCard
				name="Registry Credentials"
				icon={Package}
				status={registryStatus()}
				details={registryDetails()}
				href="/integrations/registry"
			/>
		</div>
	</section>
</div>
