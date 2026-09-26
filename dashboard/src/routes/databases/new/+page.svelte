<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createDatabase, type Project } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import Database from '~icons/ri/database-2-line';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import DbWizardConfig from '$lib/components/wizard/db-wizard-config.svelte';
	import type { DbType } from '$lib/components/wizard/db-wizard-config.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const projectIdFromQuery = $derived(page.url.searchParams.get('project_id'));

	let loading = $state(false);
	let error = $state('');
	let projects = $derived(data.projects as Project[]);

	let name = $state('');
	let dbType = $state<DbType>('postgres');
	let selectedProjectId = $state<string>('');

	$effect(() => {
		setUiContext(projectsContext());
		if (projectIdFromQuery) {
			selectedProjectId = projectIdFromQuery;
		}
		return () => clearUiContext();
	});

	async function handleCreate() {
		if (!name.trim()) {
			error = 'Database name is required';
			return;
		}
		error = '';
		loading = true;
		try {
			const db = await createDatabase({
				name: name.trim(),
				db_type: dbType,
				...(selectedProjectId ? { project_id: selectedProjectId } : {})
			});
			goto(`/databases/${db.id}`);
		} catch (e: any) {
			error = e.message ?? 'Failed to create database';
			loading = false;
		}
	}
</script>

<PageTitle title="New database" />

<div class="space-y-6 pb-12">
	<Breadcrumb items={[{ label: 'Databases', href: '/databases' }, { label: 'New database' }]} />
	<h1 class="text-2xl font-extrabold sm:text-3xl">New database</h1>

	<InlineError message={error} />

	<DbWizardConfig bind:name bind:dbType bind:selectedProjectId {projects} />

	<div>
		<Button
			onclick={handleCreate}
			disabled={loading}
			variant="outline"
			class="border-success text-success"
		>
			{#if loading}
				<span class="animate-pulse">Provisioning...</span>
			{:else}
				<Database class="mr-2 h-4 w-4" />
				Provision Database
			{/if}
		</Button>
	</div>
</div>
