<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { page } from '$app/state';
	import { pollWhileVisible } from '$lib/polling';
	import {
		getProject,
		updateProject,
		deployProject,
		deleteProject,
		assignAppProject,
		listDatabases,
		listApps,
		listProjectPreviews,
		deleteProjectPreview,
		type ProjectDetail,
		type App,
		type ManagedDatabase,
		type ProjectPreview
	} from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { goto } from '$app/navigation';
	import {
		setUiContext,
		clearUiContext,
		projectContext,
		openFeedPanel
	} from '$lib/ui-context.svelte';
	import ProjectBlueprint from '$lib/components/cards/project-blueprint/project-blueprint.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ConfirmDialog from '$lib/components/dialogs/confirm-dialog.svelte';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import CreateProjectPreviewDialog from '$lib/components/dialogs/create-project-preview-dialog.svelte';
	import PreviewDomainRequiredAlert from '$lib/components/alerts/preview-domain-required-alert.svelte';
	import ProjectPreviewCard from '$lib/components/cards/project-preview-card/project-preview-card.svelte';
	import Plus from '~icons/ri/add-line';
	import ChevronUp from '~icons/ri/arrow-up-s-line';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import type { PageProps } from './$types';

	let { data, params }: PageProps = $props();
	type ProjectDetailData = {
		project: ProjectDetail;
		allDatabases: ManagedDatabase[];
		allApps: App[];
		projectPreviews: ProjectPreview[];
		previewDomain: string;
	};

	let projectId = $derived(page.params.id || params.id);

	let project = $state<ProjectDetail | null>(null);
	let allDatabases = $state<ManagedDatabase[]>([]);
	let allApps = $state<App[]>([]);
	let error = $state('');

	// Edit state
	let editing = $state(false);
	let editName = $state('');
	let editDescription = $state('');
	let editError = $state('');
	let saving = $state(false);

	// Deploy state
	let deploying = $state(false);

	// Assign state
	let assigningId = $state<string | null>(null);

	// Delete dialog
	let showDeleteDialog = $state(false);
	let deleting = $state(false);

	// Unassigned apps panel
	let showUnassigned = $state(false);

	// Preview environments
	let projectPreviews = $state<ProjectPreview[]>([]);
	let previewsLoading = $state(false);
	let previewsError = $state('');
	let showCreatePreview = $state(false);
	let previewDomain = $state('');
	let deletingPreviewId = $state<string | null>(null);

	async function refreshProjectData() {
		error = '';
		try {
			[project, allDatabases, allApps] = await Promise.all([
				getProject(projectId),
				listDatabases(),
				listApps()
			]);
			editName = project.name;
			editDescription = project.description ?? '';
		} catch (e: any) {
			error = e.message ?? 'Failed to load project';
		}
	}

	async function loadPreviews() {
		previewsLoading = true;
		previewsError = '';
		try {
			projectPreviews = await listProjectPreviews(projectId);
		} catch (e: any) {
			previewsError = e.message ?? 'Failed to load previews';
		} finally {
			previewsLoading = false;
		}
	}

	async function handleSave() {
		if (!project) return;
		editError = '';
		if (!editName.trim()) {
			editError = 'Name is required';
			return;
		}
		saving = true;
		try {
			await updateProject(project.id, {
				name: editName.trim(),
				...(editDescription.trim() ? { description: editDescription.trim() } : {})
			});
			editing = false;
			await refreshProjectData();
		} catch (e: any) {
			editError = e.message ?? 'Failed to update project';
		} finally {
			saving = false;
		}
	}

	async function handleDeploy() {
		if (!project) return;
		deploying = true;
		openFeedPanel();
		try {
			await deployProject(project.id);
		} catch (e: any) {
			error = e.message ?? 'Failed to deploy project';
		} finally {
			deploying = false;
		}
	}

	// Throws so ConfirmDialog shows the error inside the dialog.
	async function handleDelete() {
		if (!project) return;
		deleting = true;
		try {
			await deleteProject(project.id);
			goto('/projects', { invalidateAll: true });
		} finally {
			deleting = false;
		}
	}

	async function handleRemoveApp(appId: string) {
		assigningId = appId;
		try {
			await assignAppProject(appId, null);
			await refreshProjectData();
		} catch (e: any) {
			error = e.message ?? 'Failed to remove app from project';
		} finally {
			assigningId = null;
		}
	}

	async function handleAddApp(appId: string) {
		assigningId = appId;
		try {
			await assignAppProject(appId, projectId);
			await refreshProjectData();
		} catch (e: any) {
			error = e.message ?? 'Failed to add app to project';
		} finally {
			assigningId = null;
		}
	}

	function handleDeletePreview(previewId: string) {
		previewsError = '';
		return confirmAction({
			title: 'Delete this project preview?',
			description:
				"All of the preview's app containers are stopped and removed, and its URLs stop working.",
			confirmLabel: 'Delete preview',
			pendingLabel: 'Deleting…',
			onconfirm: async () => {
				deletingPreviewId = previewId;
				try {
					await deleteProjectPreview(projectId, previewId);
					await loadPreviews();
				} finally {
					deletingPreviewId = null;
				}
			}
		});
	}

	function formatDate(dt: string) {
		return new Date(dt).toLocaleString();
	}

	// Databases assigned to this project
	let projectDatabases = $derived(allDatabases.filter((d) => d.project_id === (project?.id ?? '')));

	// Unassigned apps
	let unassignedApps = $derived(
		allApps.filter((a) => !a.project_id && !project?.apps.find((pa) => pa.id === a.id))
	);

	$effect(() => {
		const routeData = data as ProjectDetailData;
		project = routeData.project;
		allDatabases = routeData.allDatabases;
		allApps = routeData.allApps;
		projectPreviews = routeData.projectPreviews;
		previewDomain = routeData.previewDomain ?? '';
		if (project) {
			editName = project.name;
			editDescription = project.description ?? '';
			setUiContext(projectContext(project));
		}
	});

	$effect(() => {
		// Re-run whenever projectId changes (navigation between projects)
		const id = projectId;
		void id; // declare dependency

		// Poll previews every 5 s while any preview is building (paused in hidden tabs)
		const stopPolling = pollWhileVisible(
			() => {
				const hasBuilding = projectPreviews.some(
					(pp) => pp.status === 'building' || pp.previews.some((p) => p.status === 'building')
				);
				if (hasBuilding) return loadPreviews();
			},
			5000,
			{ immediate: false }
		);

		return () => {
			clearUiContext();
			stopPolling();
		};
	});
</script>

<PageTitle title={project?.name ?? 'Project'} />

<div class="space-y-6">
	<!-- Breadcrumb -->
	<Breadcrumb
		items={[{ label: 'Projects', href: '/projects' }, { label: project?.name ?? 'Loading...' }]}
	/>

	<InlineError message={error} />

	{#if project}
		<!-- Blueprint diagram -->
		<ProjectBlueprint projectName={project.name} apps={project.apps} databases={projectDatabases} />

		<!-- Header -->
		<div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
			<div class="min-w-0">
				<h1 class="text-2xl font-extrabold break-words sm:text-3xl">{project.name}</h1>
				{#if project.description}
					<p class="text-sm text-muted-foreground">{project.description}</p>
				{/if}
				<p class="text-xs text-muted-foreground">
					Created {formatDate(project.created_at)} · Updated {formatDate(project.updated_at)}
				</p>
			</div>
			<div class="flex shrink-0 flex-wrap gap-2">
				<Button
					variant="outline"
					size="sm"
					onclick={() => {
						editing = !editing;
						if (editing) {
							editName = project!.name;
							editDescription = project!.description ?? '';
						}
					}}
				>
					{editing ? 'Cancel' : 'Edit'}
				</Button>
				<Button size="sm" disabled={deploying} aria-busy={deploying} onclick={handleDeploy}>
					{deploying ? 'Deploying…' : 'Deploy all'}
				</Button>
			</div>
		</div>

		<!-- Edit form -->
		{#if editing}
			<Card.Root>
				<Card.Header>
					<Card.Title>Edit Project</Card.Title>
				</Card.Header>
				<Card.Content>
					<div class="grid gap-4 sm:grid-cols-2">
						<div class="flex flex-col gap-1.5">
							<Label for="edit-name">Name</Label>
							<Input id="edit-name" bind:value={editName} />
						</div>
						<div class="flex flex-col gap-1.5">
							<Label for="edit-desc">Description</Label>
							<Input id="edit-desc" bind:value={editDescription} placeholder="Optional" />
						</div>
					</div>
					<InlineError message={editError} />
				</Card.Content>
				<Card.Footer class="flex justify-end gap-2">
					<Button variant="outline" onclick={() => (editing = false)}>Cancel</Button>
					<Button onclick={handleSave} disabled={saving}>
						{saving ? 'Saving…' : 'Save'}
					</Button>
				</Card.Footer>
			</Card.Root>
		{/if}

		<!-- Preview Environments -->
		<Card.Root>
			<Card.Header>
				<div class="flex items-center justify-between">
					<div>
						<Card.Title>Preview Environments ({projectPreviews.length})</Card.Title>
						<Card.Description>
							Ephemeral environments with one preview deployment per member app, sharing a URL
							prefix.
						</Card.Description>
					</div>
					<Button
						size="sm"
						variant="outline"
						onclick={() => (showCreatePreview = true)}
						disabled={project.apps.length === 0 || !previewDomain}
					>
						<Plus class="mr-1 h-4 w-4" />
						New Preview
					</Button>
				</div>
			</Card.Header>
			<Card.Content>
				{#if !previewDomain}
					<div class="mb-4">
						<PreviewDomainRequiredAlert />
					</div>
				{/if}
				<InlineError message={previewsError} />

				{#if previewsLoading && projectPreviews.length === 0}
					<div class="py-8 text-center text-muted-foreground">Loading...</div>
				{:else if projectPreviews.length === 0}
					<div class="rounded-lg border border-dashed py-10 text-center">
						<p class="text-sm text-muted-foreground">No active preview environments</p>
						{#if project.apps.length === 0}
							<p class="mt-1 text-xs text-muted-foreground">
								Add member apps first before creating a preview
							</p>
						{:else if previewDomain}
							<Button
								variant="outline"
								size="sm"
								class="mt-3"
								onclick={() => (showCreatePreview = true)}
							>
								<Plus class="mr-1 h-4 w-4" />
								Create Preview
							</Button>
						{/if}
					</div>
				{:else}
					<div class="flex flex-col gap-2">
						{#each projectPreviews as pp (pp.id)}
							<ProjectPreviewCard
								preview={pp}
								apps={allApps}
								onDelete={handleDeletePreview}
								deleting={deletingPreviewId === pp.id}
							/>
						{/each}
					</div>
				{/if}
			</Card.Content>
		</Card.Root>

		<!-- Member apps -->
		<Card.Root>
			<Card.Header>
				<div class="flex items-center justify-between">
					<div>
						<Card.Title>Member Apps ({project.apps.length})</Card.Title>
						<Card.Description>Apps assigned to this project.</Card.Description>
					</div>
					<Button size="sm" href={`/apps/new?project_id=${projectId}`}>
						<Plus class="mr-1 h-4 w-4" />
						Add App
					</Button>
				</div>
			</Card.Header>
			<Card.Content>
				{#if project.apps.length === 0}
					<div class="rounded-lg border border-dashed py-8 text-center">
						<p class="text-sm text-muted-foreground">No apps assigned yet.</p>
						<p class="mt-1 text-xs text-muted-foreground">
							Create a new app or add from unassigned apps below
						</p>
					</div>
				{:else}
					<div class="flex flex-col gap-2">
						{#each project.apps as app (app.id)}
							<div class="flex items-center justify-between rounded-md border p-3">
								<div class="flex items-center gap-3">
									<a href={`/apps/${app.id}`} class="font-medium hover:underline">{app.name}</a>
									<StatusBadge status={app.status} kind="app" size="sm" />
								</div>
								<Button
									variant="outline"
									size="sm"
									disabled={assigningId === app.id}
									onclick={() => handleRemoveApp(app.id)}
								>
									Remove
								</Button>
							</div>
						{/each}
					</div>
				{/if}

				<!-- Unassigned apps section -->
				{#if unassignedApps.length > 0}
					<div class="mt-6 border-t pt-4">
						<button
							onclick={() => (showUnassigned = !showUnassigned)}
							class="flex items-center gap-2 text-sm font-medium hover:text-foreground"
						>
							{#if showUnassigned}
								<ChevronUp class="h-4 w-4" />
							{:else}
								<ChevronRight class="h-4 w-4" />
							{/if}
							Unassigned Apps ({unassignedApps.length})
						</button>
						{#if showUnassigned}
							<div class="mt-3 flex flex-col gap-2">
								{#each unassignedApps as app (app.id)}
									<div class="flex items-center justify-between rounded-md border bg-muted/30 p-3">
										<div class="flex items-center gap-3">
											<a href={`/apps/${app.id}`} class="font-medium hover:underline">{app.name}</a>
											<StatusBadge status={app.status} kind="app" size="sm" />
										</div>
										<Button
											size="sm"
											disabled={assigningId === app.id}
											onclick={() => handleAddApp(app.id)}
										>
											Add to Project
										</Button>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
			</Card.Content>
		</Card.Root>

		<section
			aria-labelledby="project-danger-heading"
			class="space-y-3 rounded-sm border border-destructive/40 p-4"
		>
			<h2 id="project-danger-heading" class="text-sm font-semibold text-destructive">
				Danger zone
			</h2>
			<p class="text-sm text-muted-foreground">
				Delete the project. Its apps are kept and become unassigned.
			</p>
			<Button
				variant="destructive"
				size="sm"
				disabled={deleting}
				onclick={() => (showDeleteDialog = true)}
			>
				Delete project…
			</Button>
		</section>
	{/if}
</div>

<ConfirmDialog
	bind:open={showDeleteDialog}
	title={`Delete project ${project?.name ?? ''}?`}
	description="The project and its shared environment variables are deleted. Its apps are kept and become unassigned; nothing is stopped."
	confirmLabel="Delete project"
	pendingLabel="Deleting…"
	onconfirm={handleDelete}
/>

<CreateProjectPreviewDialog
	bind:open={showCreatePreview}
	{projectId}
	oncreated={() => loadPreviews()}
/>
