<script lang="ts">
	import { updateSettings } from '$lib/api';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SettingsForm from '$lib/components/forms/settings-form.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let error = $state('');

	// Form state — mirrors settings fields, populated once loaded
	let serverName = $state('');
	let dashboardDomain = $state('');

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
				dashboard_domain: dashboardDomain
			});
			serverName = updated.server_name;
			dashboardDomain = updated.dashboard_domain;
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
		isSaving={savingAccess}
		saved={accessSaved}
		error={accessError}
		onSubmit={handleSaveAccess}
	/>
</div>
