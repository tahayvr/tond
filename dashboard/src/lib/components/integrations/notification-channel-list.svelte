<script lang="ts">
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import {
		listIntegrations,
		createIntegration,
		updateIntegration,
		deleteIntegration,
		testIntegration,
		type Integration,
		type IntegrationKind,
		type IntegrationConfig
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Plus from '~icons/ri/add-line';
	import Pencil from '~icons/ri/pencil-line';
	import FlaskConical from '~icons/ri/flask-line';

	interface Props {
		kind: IntegrationKind;
		initialIntegrations?: Integration[];
		/** Human-readable channel name shown in the UI */
		label: string;
		/** Slot to render the channel-specific config fields */
		configFields?: import('svelte').Snippet;
		/** Build config object from the current form values */
		buildConfig: () => IntegrationConfig;
		/** Reset channel-specific form fields to empty */
		resetConfig: () => void;
		/** Populate channel-specific form fields from an existing config (for editing) */
		loadConfig: (config: IntegrationConfig) => void;
	}

	let {
		kind,
		initialIntegrations,
		label,
		configFields,
		buildConfig,
		resetConfig,
		loadConfig
	}: Props = $props();
	let initialized = $state(false);

	// ── State ─────────────────────────────────────────────────────────────────

	let integrations = $state<Integration[]>([]);
	let loading = $state(true);
	let error = $state('');
	let deletingId = $state<string | null>(null);
	let testingId = $state<string | null>(null);
	let testMessage = $state('');

	// ── Add form ──────────────────────────────────────────────────────────────

	let showAdd = $state(false);
	let addName = $state('');
	let adding = $state(false);
	let addError = $state('');

	// ── Edit form ─────────────────────────────────────────────────────────────

	let editingId = $state<string | null>(null);
	let editName = $state('');
	let saving = $state(false);
	let editError = $state('');

	// ── Load ──────────────────────────────────────────────────────────────────

	async function load() {
		loading = true;
		error = '';
		try {
			const all = await listIntegrations();
			integrations = all.filter((i) => i.kind === kind);
		} catch (e: any) {
			error = e.message ?? `Failed to load ${label} integrations`;
		} finally {
			loading = false;
		}
	}

	// ── Actions ───────────────────────────────────────────────────────────────

	async function handleAdd() {
		addError = '';
		if (!addName.trim()) {
			addError = 'Name is required';
			return;
		}
		let config: IntegrationConfig;
		try {
			config = buildConfig();
		} catch (e: any) {
			addError = e.message ?? 'Invalid configuration';
			return;
		}

		adding = true;
		try {
			const created = await createIntegration({ name: addName.trim(), kind, config });
			integrations = [created, ...integrations];
			addName = '';
			resetConfig();
			showAdd = false;
		} catch (e: any) {
			addError = e.message ?? 'Failed to add integration';
		} finally {
			adding = false;
		}
	}

	function openEdit(integration: Integration) {
		// Close add form if open
		showAdd = false;
		addName = '';
		addError = '';
		resetConfig();

		editingId = integration.id;
		editName = integration.name;
		editError = '';
		loadConfig(integration.config);
	}

	async function handleSave() {
		if (!editingId) return;
		editError = '';
		if (!editName.trim()) {
			editError = 'Name is required';
			return;
		}
		let config: IntegrationConfig;
		try {
			config = buildConfig();
		} catch (e: any) {
			editError = e.message ?? 'Invalid configuration';
			return;
		}

		saving = true;
		try {
			const updated = await updateIntegration(editingId, { name: editName.trim(), config });
			integrations = integrations.map((i) => (i.id === updated.id ? updated : i));
			cancelEdit();
		} catch (e: any) {
			editError = e.message ?? 'Failed to save changes';
		} finally {
			saving = false;
		}
	}

	function cancelEdit() {
		editingId = null;
		editName = '';
		editError = '';
		resetConfig();
	}

	async function handleDelete(id: string) {
		const target = integrations.find((i) => i.id === id);
		error = '';
		await confirmAction({
			title: `Delete ${target?.name ?? 'this channel'}?`,
			description:
				'Alert rules that notify this channel stop sending to it. Rules left without any channel stay saved but notify nobody until you add one.',
			confirmLabel: 'Delete channel',
			pendingLabel: 'Deleting…',
			onconfirm: async () => {
				deletingId = id;
				try {
					await deleteIntegration(id);
					integrations = integrations.filter((i) => i.id !== id);
					if (editingId === id) cancelEdit();
				} finally {
					deletingId = null;
				}
			}
		});
	}

	async function handleToggle(integration: Integration) {
		try {
			const updated = await updateIntegration(integration.id, { enabled: !integration.enabled });
			integrations = integrations.map((i) => (i.id === updated.id ? updated : i));
		} catch (e: any) {
			error = e.message ?? 'Failed to update integration';
		}
	}

	async function handleTest(id: string) {
		testingId = id;
		testMessage = '';
		error = '';
		try {
			const result = await testIntegration(id);
			testMessage = result.message;
		} catch (e: any) {
			error = e.message ?? 'Test notification failed';
		} finally {
			testingId = null;
		}
	}

	function cancelAdd() {
		showAdd = false;
		addError = '';
		addName = '';
		resetConfig();
	}

	function openAdd() {
		// Close edit form if open
		cancelEdit();
		showAdd = true;
	}

	function formatDate(dt: string) {
		return new Date(dt).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	$effect(() => {
		if (initialized) return;
		initialized = true;

		if (initialIntegrations !== undefined) {
			integrations = initialIntegrations;
			loading = false;
			return;
		}

		void load();
	});
</script>

<InlineError title="Integration Error" message={error} />

<SuccessAlert message={testMessage} />

<div class="space-y-3">
	<div class="flex items-center justify-between">
		<p class="text-sm text-muted-foreground">
			Named {label} channels used for alert notifications.
		</p>
		{#if !showAdd}
			<Button size="sm" class="h-7 text-xs uppercase" onclick={openAdd}>
				<Plus class="h-3 w-3" /> Add Channel
			</Button>
		{/if}
	</div>

	{#if showAdd}
		<form
			onsubmit={(e) => {
				e.preventDefault();
				handleAdd();
			}}
			class="space-y-3 border bg-muted/30 p-4"
		>
			<p class="text-xs font-semibold uppercase">New {label} Channel</p>
			<InlineError message={addError} />
			<div class="grid gap-1.5">
				<Label class="text-xs" for="ch-name">Display Name</Label>
				<Input
					id="ch-name"
					class="h-8 text-xs"
					bind:value={addName}
					placeholder="e.g. My {label} Channel"
				/>
			</div>
			{#if configFields}
				{@render configFields()}
			{/if}
			<div class="flex gap-2 pt-1">
				<Button type="submit" size="sm" class="h-8 text-xs uppercase" disabled={adding}>
					{adding ? 'Saving…' : 'Save Channel'}
				</Button>
				<Button
					type="button"
					variant="outline"
					size="sm"
					class="h-8 text-xs uppercase"
					onclick={cancelAdd}
				>
					Cancel
				</Button>
			</div>
		</form>
	{/if}

	{#if loading}
		<p class="py-4 text-center text-xs text-muted-foreground">Loading…</p>
	{:else if integrations.length === 0 && !showAdd}
		<div class="border border-dashed py-10 text-center text-xs text-muted-foreground">
			No {label} channels configured yet. Add one to start receiving notifications.
		</div>
	{:else}
		<div class="space-y-px">
			{#each integrations as integration (integration.id)}
				<div class="border bg-background text-xs">
					<div class="flex items-center justify-between px-4 py-3">
						<div class="flex min-w-0 flex-col gap-0.5">
							<div class="flex items-center gap-2">
								<span class="font-medium">{integration.name}</span>
								{#if integration.enabled}
									<StatusBadge status="enabled" size="sm" />
								{:else}
									<StatusBadge status="disabled" size="sm" />
								{/if}
							</div>
							<span class="text-muted-foreground">Added {formatDate(integration.created_at)}</span>
						</div>
						<div class="ml-4 flex shrink-0 items-center gap-1">
							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-xs uppercase"
								disabled={testingId === integration.id}
								onclick={() => handleTest(integration.id)}
							>
								<FlaskConical class="h-3.5 w-3.5" />
								{testingId === integration.id ? 'Sending…' : 'Test'}
							</Button>
							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-xs uppercase"
								onclick={() => handleToggle(integration)}
							>
								{integration.enabled ? 'Disable' : 'Enable'}
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								aria-label="Edit"
								disabled={editingId === integration.id}
								onclick={() => openEdit(integration)}
							>
								<Pencil class="h-3.5 w-3.5" />
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-destructive hover:text-destructive"
								aria-label={`Delete ${integration.name}`}
								disabled={deletingId === integration.id}
								onclick={() => handleDelete(integration.id)}
							>
								<Trash2 class="h-3.5 w-3.5" />
							</Button>
						</div>
					</div>

					{#if editingId === integration.id}
						<form
							onsubmit={(e) => {
								e.preventDefault();
								handleSave();
							}}
							class="space-y-3 border-t bg-muted/30 px-4 py-4"
						>
							<p class="text-xs font-semibold uppercase">Edit {label} Channel</p>
							<InlineError message={editError} />
							<div class="grid gap-1.5">
								<Label class="text-xs" for="edit-ch-name">Display Name</Label>
								<Input id="edit-ch-name" class="h-8 text-xs" bind:value={editName} />
							</div>
							{#if configFields}
								{@render configFields()}
							{/if}
							<div class="flex gap-2 pt-1">
								<Button type="submit" size="sm" class="h-8 text-xs uppercase" disabled={saving}>
									{saving ? 'Saving…' : 'Save Changes'}
								</Button>
								<Button
									type="button"
									variant="outline"
									size="sm"
									class="h-8 text-xs uppercase"
									onclick={cancelEdit}
								>
									Cancel
								</Button>
							</div>
						</form>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>
