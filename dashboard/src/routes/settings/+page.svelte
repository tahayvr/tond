<script lang="ts">
	import { updateSettings } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SettingsForm from '$lib/components/forms/settings-form.svelte';
	import SessionSettingsForm from '$lib/components/forms/session-settings-form.svelte';
	import { can } from '$lib/auth.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let error = $state('');
	const MIB = 1024 * 1024;

	// Form state — mirrors settings fields, populated once loaded
	let serverName = $state('');
	let dashboardDomain = $state('');
	let previewDomain = $state('');
	let appsDomain = $state('');
	/** Upload limit in MiB (the API takes bytes) */
	let uploadLimitMib = $state(1024);
	// Last saved dashboard domain (drives the certificate status line)
	let savedDashboardDomain = $state('');
	let certRefreshKey = $state(0);

	// Per-section save state
	let savingAccess = $state(false);
	let accessSaved = $state(false);
	let accessError = $state('');

	async function handleSaveAccess() {
		savingAccess = true;
		accessSaved = false;
		accessError = '';
		try {
			const updated = await updateSettings({
				server_name: serverName,
				dashboard_domain: dashboardDomain,
				preview_domain: previewDomain,
				apps_domain: appsDomain,
				upload_max_bytes: Math.round(uploadLimitMib * MIB)
			});
			serverName = updated.server_name;
			dashboardDomain = updated.dashboard_domain;
			previewDomain = updated.preview_domain;
			appsDomain = updated.apps_domain;
			uploadLimitMib = Math.round(updated.upload_max_bytes / MIB);
			savedDashboardDomain = updated.dashboard_domain;
			certRefreshKey += 1;
			accessSaved = true;
			setTimeout(() => (accessSaved = false), 3000);
		} catch (e: any) {
			accessError = e.message;
		} finally {
			savingAccess = false;
		}
	}

	$effect(() => {
		serverName = data.settings.server_name;
		dashboardDomain = data.settings.dashboard_domain;
		previewDomain = data.settings.preview_domain ?? '';
		appsDomain = data.settings.apps_domain ?? '';
		uploadLimitMib = Math.round((data.settings.upload_max_bytes ?? 1024 * MIB) / MIB);
		savedDashboardDomain = data.settings.dashboard_domain;
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex h-14 items-center justify-between">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Settings</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">SYSTEM_CONFIGURATION</p>
		</div>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-4 text-destructive">
			{error}
		</div>
	{/if}

	<SettingsForm
		bind:serverName
		bind:dashboardDomain
		bind:previewDomain
		bind:appsDomain
		bind:uploadLimitMib
		{savedDashboardDomain}
		{certRefreshKey}
		isSaving={savingAccess}
		saved={accessSaved}
		error={accessError}
		onSubmit={handleSaveAccess}
	/>

	<SessionSettingsForm settings={data.settings} canEdit={can('settings:write')} />
</div>
