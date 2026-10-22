<script lang="ts">
	// An app deployed from a template: which template and version it runs,
	// "Update available" with the new version's upgrade notes and a one-click
	// upgrade, and env values its domain change left on the old address.
	import { goto } from '$app/navigation';
	import { can } from '$lib/auth.svelte';
	import { getAppTemplate, upgradeAppTemplate, type App, type TemplateAppStatus } from '$lib/api';
	import { confirmAction } from '$lib/confirm.svelte';
	import { offerTemplateDomainUpdate } from '$lib/template-app';
	import { Button } from '$lib/components/ui/button/index.js';
	import LayoutTemplate from '~icons/ri/layout-masonry-line';
	import UpgradeIcon from '~icons/ri/arrow-up-circle-line';
	import InfoIcon from '~icons/ri/information-line';

	interface Props {
		app: App;
	}

	let { app }: Props = $props();

	let status = $state<TemplateAppStatus | null>(null);

	async function load() {
		try {
			status = await getAppTemplate(app.id);
		} catch {
			status = null;
		}
	}

	$effect(() => {
		void app.id;
		void app.domain;
		void app.template_version;
		load();
	});

	let canUpgrade = $derived(can('settings:write') && can('apps:deploy'));

	async function handleUpgrade() {
		if (!status?.latest_version) return;
		const to = status.latest_version;
		await confirmAction({
			title: `Upgrade ${app.name} to ${status.template_name ?? status.template_id} ${to}?`,
			description:
				'Read the upgrade notes first. You can roll back to the current deployment from the Deployments tab, and restore the snapshot if the new version changed the data.',
			details: [
				"A snapshot of the app's data is taken first; nothing changes if it fails",
				`The compose file is replaced with version ${to}'s`,
				'Environment variables and generated passwords are kept; settings the new version adds are filled in',
				'A deploy starts right away'
			],
			confirmLabel: `Upgrade to ${to}`,
			pendingLabel: 'Taking a snapshot…',
			destructive: false,
			onconfirm: async () => {
				const result = await upgradeAppTemplate(app.id);
				await goto(`/apps/${app.id}/deployments/${result.deployment_id}`);
			}
		});
	}

	async function handleDomainEnv() {
		if (await offerTemplateDomainUpdate(app.id, app.name)) await load();
	}
</script>

<section class="space-y-2" aria-label="Template">
	<p class="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-muted-foreground">
		<LayoutTemplate class="size-4 shrink-0" aria-hidden="true" />
		<span>
			Deployed from the
			<a
				href="/templates/{app.template_id}"
				class="text-foreground underline-offset-4 hover:underline">{app.template_id}</a
			>
			template{#if app.template_version}&nbsp;({app.template_version}){/if}
		</span>
		<span aria-hidden="true">·</span>
		<a
			href="/templates/{app.template_id}#upgrading"
			class="text-foreground underline-offset-4 hover:underline">Upgrade notes</a
		>
	</p>

	{#if status?.update_available}
		<div class="space-y-2 rounded-sm border border-info/40 bg-info/10 px-3 py-2.5 text-sm">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<p class="flex items-center gap-2 font-medium">
					<UpgradeIcon class="size-4 shrink-0" aria-hidden="true" />
					Update available: {status.version ?? '?'} → {status.latest_version}
				</p>
				{#if canUpgrade && !status.upgrade_blocked}
					<Button size="sm" onclick={handleUpgrade}>Upgrade to {status.latest_version}</Button>
				{/if}
			</div>
			{#if status.upgrade_notes}
				<p class="whitespace-pre-line text-muted-foreground">{status.upgrade_notes}</p>
			{/if}
			{#if status.upgrade_blocked}
				<p class="flex items-start gap-2 text-muted-foreground">
					<InfoIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
					<span>{status.upgrade_blocked}</span>
				</p>
			{/if}
		</div>
	{/if}

	{#if status && status.domain_env.length > 0}
		<div
			class="flex flex-wrap items-center justify-between gap-3 rounded-sm border border-warn/50 bg-warn/10 px-3 py-2.5 text-sm"
			role="status"
		>
			<span class="flex items-start gap-2">
				<InfoIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
				<span>
					{status.domain_env.length === 1
						? `${status.domain_env[0].key} still uses`
						: `${status.domain_env.length} settings still use`} the app's old address.
				</span>
			</span>
			{#if can('env:write')}
				<Button size="sm" variant="outline" onclick={handleDomainEnv}>Update and redeploy…</Button>
			{/if}
		</div>
	{/if}
</section>
