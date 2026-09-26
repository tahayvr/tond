<script lang="ts">
	import { updateSettings } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SettingsForm from '$lib/components/forms/settings-form.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let error = $state('');

	// Form state — mirrors settings fields, populated once loaded
	let serverName = $state('');
	let dashboardDomain = $state('');
	let previewDomain = $state('');
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
				preview_domain: previewDomain
			});
			serverName = updated.server_name;
			dashboardDomain = updated.dashboard_domain;
			previewDomain = updated.preview_domain;
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
		savedDashboardDomain = data.settings.dashboard_domain;
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader title="Settings" description="Server name, dashboard domain and preview domain." />

	<InlineError message={error} ondismiss={() => (error = '')} />

	<SettingsForm
		bind:serverName
		bind:dashboardDomain
		bind:previewDomain
		{savedDashboardDomain}
		{certRefreshKey}
		isSaving={savingAccess}
		saved={accessSaved}
		error={accessError}
		onSubmit={handleSaveAccess}
	/>
</div>
