<script lang="ts">
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
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { goto } from '$app/navigation';
	import {
		setUiContext,
		clearUiContext,
		projectContext,
		openFeedPanel
	} from '$lib/ui-context.svelte';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import ProjectBlueprint from '$lib/components/cards/project-blueprint/project-blueprint.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import ConfirmDeleteDialog from '$lib/components/dialogs/confirm-delete-dialog.svelte';
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

	async function handleDelete() {
		if (!project) return;
		deleting = true;
		try {
			await deleteProject(project.id);
			goto('/projects');
		} catch (e: any) {
			error = e.message ?? 'Failed to delete project';
			deleting = false;
			showDeleteDialog = false;
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

	async function handleDeletePreview(previewId: string) {
		deletingPreviewId = previewId;
		previewsError = '';
		try {
			await deleteProjectPreview(projectId, previewId);
			await loadPreviews();
		} catch (e: any) {
			previewsError = e.message ?? 'Failed to delete preview';
		} finally {
			deletingPreviewId = null;
		}
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

<div class="space-y-6">
	<!-- Breadcrumb -->
	<Breadcrumb
		items={[{ label: 'Projects', href: '/projects' }, { label: project?.name ?? 'Loading...' }]}
	/>

	{#if error}
		<ErrorAlert title="Error" description={error} />
	{/if}

	{#if project}
		<!-- Blueprint diagram -->
		<ProjectBlueprint projectName={project.name} apps={project.apps} databases={projectDatabases} />

		<!-- Header -->
		<div class="flex items-start justify-between gap-4">
			<div>
				<h3 class="text-xl font-semibold">{project.name}</h3>
				{#if project.description}
					<p class="text-sm text-muted-foreground">{project.description}</p>
				{/if}
				<p class="text-xs text-muted-foreground">
					Created {formatDate(project.created_at)} · Updated {formatDate(project.updated_at)}
				</p>
			</div>
			<div class="flex shrink-0 gap-2">
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
				<Button size="sm" disabled={deploying} onclick={handleDeploy}>
					{deploying ? 'Deploying…' : 'Deploy All'}
				</Button>
				<Button
					variant="destructive"
					size="sm"
					disabled={deleting}
					onclick={() => (showDeleteDialog = true)}
				>
					Delete
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
					{#if editError}
						<p class="mt-3 text-sm text-destructive">{editError}</p>
					{/if}
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
				{#if previewsError}
					<div
						class="mb-4 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-sm text-destructive"
					>
						{previewsError}
					</div>
				{/if}

				{#if previewsLoading && projectPreviews.length === 0}
					<div class="py-8 text-center text-muted-foreground">Loading...</div>
				{:else if projectPreviews.length === 0}
					<div class="rounded-lg border border-dashed py-10 text-center">
						<p class="text-sm text-muted-foreground">No active preview environments</p>
						{#if project.apps.length === 0}
							<p class="mt-1 text-xs text-muted-foreground/60">
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
					<Button size="sm" onclick={() => goto(`/apps/new?project_id=${projectId}`)}>
						<Plus class="mr-1 h-4 w-4" />
						Add App
					</Button>
				</div>
			</Card.Header>
			<Card.Content>
				{#if project.apps.length === 0}
					<div class="rounded-lg border border-dashed py-8 text-center">
						<p class="text-sm text-muted-foreground">No apps assigned yet.</p>
						<p class="mt-1 text-xs text-muted-foreground/60">
							Create a new app or add from unassigned apps below
						</p>
					</div>
				{:else}
					<div class="flex flex-col gap-2">
						{#each project.apps as app (app.id)}
							<div class="flex items-center justify-between rounded-md border p-3">
								<div class="flex items-center gap-3">
									<button
										class="font-medium hover:underline"
										onclick={() => goto(`/apps/${app.id}`)}
									>
										{app.name}
									</button>
									<Badge
										variant={app.status === 'running'
											? 'default'
											: app.status === 'failed'
												? 'destructive'
												: 'secondary'}
									>
										{app.status}
									</Badge>
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
											<button
												class="font-medium hover:underline"
												onclick={() => goto(`/apps/${app.id}`)}
											>
												{app.name}
											</button>
											<Badge
												variant={app.status === 'running'
													? 'default'
													: app.status === 'failed'
														? 'destructive'
														: 'secondary'}
											>
												{app.status}
											</Badge>
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
	{/if}
</div>

<ConfirmDeleteDialog
	bind:open={showDeleteDialog}
	title="Delete &quot;{project?.name}&quot;?"
	description="This will permanently delete the project and all its apps. This action cannot be undone."
	confirmation={project?.name ?? ''}
	onconfirm={handleDelete}
/>

<CreateProjectPreviewDialog
	bind:open={showCreatePreview}
	{projectId}
	oncreated={() => loadPreviews()}
/>
