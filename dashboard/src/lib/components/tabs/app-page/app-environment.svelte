<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import EnvVarTable from '$lib/components/env/env-var-table.svelte';
	import EnvVarDialog, { type EnvVarDialogResult } from '$lib/components/env/env-var-dialog.svelte';
	import EnvImportDialog from '$lib/components/env/env-import-dialog.svelte';
	import { SvelteMap } from 'svelte/reactivity';
	import { untrack } from 'svelte';
	import {
		getAppEnv,
		patchAppEnv,
		patchPreviewEnv,
		revealAppEnvVar,
		revealPreviewEnvVar,
		type AppEnv,
		type RegistryCredential
	} from '$lib/api';
	import Plus from '~icons/ri/add-line';
	import FileUpload from '~icons/ri/file-upload-line';
	import Refresh from '~icons/ri/refresh-line';

	type Target = 'env' | 'preview';

	interface Props {
		appId: string;
		/** Source type of the app — registry section only shown for image sources */
		sourceType: string;
		registryCredentials: RegistryCredential[];
		registryCredentialId: string | null;
		onRegistryCredentialChange: (credentialId: string | null) => Promise<void>;
		/** Whether PR previews are on (from the app, so toggling them elsewhere shows here) */
		previewEnabled: boolean;
		/** Redeploy the app so saved changes reach the container */
		onRedeploy: () => Promise<void>;
	}

	let {
		appId,
		sourceType,
		registryCredentials,
		registryCredentialId,
		onRegistryCredentialChange,
		previewEnabled,
		onRedeploy
	}: Props = $props();

	let env = $state<AppEnv | null>(null);
	let loading = $state(true);
	let loadError = $state('');
	let actionError = $state('');
	let notice = $state('');
	/** Saved changes that the running container has not picked up yet */
	let pendingDeploy = $state(false);
	let redeploying = $state(false);

	// Plaintext values revealed on request, per target. Cleared when a value changes.
	const revealed = {
		env: new SvelteMap<string, string>(),
		preview: new SvelteMap<string, string>()
	};
	let busyKey = $state<string | null>(null);
	let copiedKey = $state<string | null>(null);
	let copyTimeout: ReturnType<typeof setTimeout> | null = null;

	// Add/edit dialog
	let dialogOpen = $state(false);
	let dialogTarget = $state<Target>('env');
	let editingKey = $state<string | null>(null);

	// Import dialog
	let importOpen = $state(false);
	let importTarget = $state<Target>('env');

	// Delete confirmation
	let deleteOpen = $state(false);
	let deleteTarget = $state<Target>('env');
	let deleteKey = $state<string | null>(null);
	let deleting = $state(false);
	let deleteError = $state('');

	let envKeys = $derived(env?.vars.map((v) => v.key) ?? []);
	let overrideKeys = $derived(env?.preview.overrides.map((v) => v.key) ?? []);
	let excluded = $derived(new Set(env?.preview.excluded_keys ?? []));
	let isImage = $derived(sourceType === 'image');
	let selectedCredentialId = $derived(registryCredentialId ?? '');

	function noun(target: Target) {
		return target === 'env' ? 'variable' : 'preview override';
	}

	async function load() {
		loading = true;
		loadError = '';
		try {
			env = await getAppEnv(appId);
		} catch (e: any) {
			loadError = e.message ?? 'Could not load environment variables';
		} finally {
			loading = false;
		}
	}

	// Load once per app; switching apps drops revealed values and pending state.
	$effect(() => {
		void appId;
		untrack(() => {
			revealed.env.clear();
			revealed.preview.clear();
			pendingDeploy = false;
			notice = '';
			actionError = '';
			load();
		});
	});

	async function reveal(target: Target, key: string): Promise<string> {
		const cached = revealed[target].get(key);
		if (cached !== undefined) return cached;
		const value =
			target === 'env' ? await revealAppEnvVar(appId, key) : await revealPreviewEnvVar(appId, key);
		revealed[target].set(key, value);
		return value;
	}

	async function handleToggleReveal(target: Target, key: string) {
		if (revealed[target].has(key)) {
			revealed[target].delete(key);
			return;
		}
		busyKey = key;
		actionError = '';
		try {
			await reveal(target, key);
		} catch (e: any) {
			actionError = e.message ?? `Could not reveal ${key}`;
		} finally {
			busyKey = null;
		}
	}

	async function handleCopy(target: Target, key: string) {
		busyKey = key;
		actionError = '';
		try {
			const wasRevealed = revealed[target].has(key);
			const value = await reveal(target, key);
			// Copying does not reveal the value on screen.
			if (!wasRevealed) revealed[target].delete(key);
			await navigator.clipboard.writeText(value);
			copiedKey = key;
			if (copyTimeout) clearTimeout(copyTimeout);
			copyTimeout = setTimeout(() => (copiedKey = null), 2000);
		} catch (e: any) {
			actionError = e.message ?? `Could not copy ${key}`;
		} finally {
			busyKey = null;
		}
	}

	/** Send a patch, then optionally redeploy. Errors propagate to the caller's dialog. */
	async function applyPatch(
		target: Target,
		patch: {
			set?: Record<string, string>;
			unset?: string[];
			exclude?: string[];
			include?: string[];
		},
		redeploy: boolean,
		message: string
	) {
		env = target === 'env' ? await patchAppEnv(appId, patch) : await patchPreviewEnv(appId, patch);
		for (const key of [...Object.keys(patch.set ?? {}), ...(patch.unset ?? [])]) {
			revealed[target].delete(key);
		}
		actionError = '';
		pendingDeploy = true;
		notice = message;
		if (redeploy) await handleRedeploy();
	}

	async function handleRedeploy() {
		redeploying = true;
		actionError = '';
		try {
			await onRedeploy();
			pendingDeploy = false;
			notice = 'Redeploy started. The new values apply once it finishes.';
		} catch (e: any) {
			actionError = `Saved, but the redeploy failed to start: ${e.message ?? 'unknown error'}`;
		} finally {
			redeploying = false;
		}
	}

	function openAdd(target: Target) {
		dialogTarget = target;
		editingKey = null;
		dialogOpen = true;
	}

	function openEdit(target: Target, key: string) {
		dialogTarget = target;
		editingKey = key;
		dialogOpen = true;
	}

	async function handleDialogSubmit({ key, value, redeploy }: EnvVarDialogResult) {
		const target = dialogTarget;
		const previousKey = editingKey;
		const renamed = previousKey !== null && previousKey !== key;

		if (previousKey !== null && value === null && !renamed) return; // nothing changed

		// Renaming without a new value carries the current value over.
		const nextValue = value ?? (await reveal(target, previousKey as string));
		const patch = renamed
			? { set: { [key]: nextValue }, unset: [previousKey as string] }
			: { set: { [key]: nextValue } };
		const verb = previousKey === null ? 'Added' : renamed ? `Renamed ${previousKey} to` : 'Updated';
		await applyPatch(target, patch, redeploy, `${verb} ${key}.`);

		// Keep a renamed variable's preview exclusion.
		if (target === 'env' && renamed && excluded.has(previousKey as string)) {
			env = await patchPreviewEnv(appId, {
				exclude: [key],
				include: [previousKey as string]
			});
		}
	}

	async function handleImport(target: Target, vars: Record<string, string>, redeploy: boolean) {
		const count = Object.keys(vars).length;
		await applyPatch(
			target,
			{ set: vars },
			redeploy,
			`Imported ${count} ${count === 1 ? noun(target) : `${noun(target)}s`}.`
		);
	}

	function openDelete(target: Target, key: string) {
		deleteTarget = target;
		deleteKey = key;
		deleteError = '';
		deleteOpen = true;
	}

	async function handleConfirmDelete() {
		if (!deleteKey) return;
		deleting = true;
		deleteError = '';
		try {
			await applyPatch(deleteTarget, { unset: [deleteKey] }, false, `Deleted ${deleteKey}.`);
			deleteOpen = false;
		} catch (e: any) {
			deleteError = e.message ?? 'Could not delete the variable';
		} finally {
			deleting = false;
		}
	}

	async function handleToggleExclude(key: string, exclude: boolean) {
		busyKey = key;
		try {
			await applyPatch(
				'preview',
				exclude ? { exclude: [key] } : { include: [key] },
				false,
				exclude
					? `${key} will no longer be passed to preview deployments.`
					: `${key} will be passed to preview deployments again.`
			);
		} catch (e: any) {
			actionError = e.message ?? `Could not update ${key}`;
		} finally {
			busyKey = null;
		}
	}

	async function handleCredentialChange(value: string) {
		actionError = '';
		try {
			await onRegistryCredentialChange(value || null);
			notice = value ? 'Registry credential attached.' : 'Registry credential removed.';
		} catch (e: any) {
			actionError = e.message ?? 'Could not update the registry credential';
		}
	}
</script>

<div class="space-y-6 pt-4">
	{#if loading && !env}
		<div class="space-y-2" aria-busy="true" aria-label="Loading environment variables">
			<Skeleton class="h-6 w-48" />
			<Skeleton class="h-24 w-full" />
		</div>
	{:else if loadError && !env}
		<div
			class="space-y-3 rounded bg-destructive/10 px-4 py-3 text-sm text-destructive"
			role="alert"
		>
			<p>Could not load environment variables: {loadError}</p>
			<p class="text-xs">Nothing was changed. Your saved variables are safe.</p>
			<Button variant="outline" size="sm" onclick={load}>
				<Refresh class="mr-2 h-3.5 w-3.5" />
				Retry
			</Button>
		</div>
	{:else if env}
		{#if pendingDeploy}
			<div
				class="flex flex-wrap items-center justify-between gap-3 rounded border border-warn/40 bg-warn/10 px-4 py-3 text-sm"
				role="status"
			>
				<div>
					<p class="font-medium text-warn">{notice || 'Changes saved.'}</p>
					<p class="text-xs text-muted-foreground">
						The running app still uses the previous values until the next deploy.
					</p>
				</div>
				<Button size="sm" onclick={handleRedeploy} disabled={redeploying}>
					{redeploying ? 'Redeploying…' : 'Redeploy now'}
				</Button>
			</div>
		{:else if notice}
			<div class="rounded bg-success/10 px-4 py-3 text-sm text-success" role="status">{notice}</div>
		{/if}

		{#if actionError}
			<div
				class="flex items-start justify-between gap-3 rounded bg-destructive/10 px-4 py-3 text-sm text-destructive"
				role="alert"
			>
				<p>{actionError}</p>
				<button
					type="button"
					class="text-xs underline-offset-2 hover:underline"
					onclick={() => (actionError = '')}
				>
					Dismiss
				</button>
			</div>
		{/if}

		<!-- Variables -->
		<section class="space-y-3">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<div>
					<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
						Environment Variables
					</h4>
					<p class="mt-0.5 text-xs text-muted-foreground">
						Values are hidden; reveal one when you need it. Each change is saved on its own and
						applies on the next deploy.
						{#if previewEnabled}Tick "Exclude" to keep a variable out of preview deployments.{/if}
					</p>
				</div>
				<div class="flex gap-2">
					<Button
						variant="outline"
						size="sm"
						onclick={() => ((importTarget = 'env'), (importOpen = true))}
					>
						<FileUpload class="mr-2 h-3.5 w-3.5" />
						Import .env
					</Button>
					<Button variant="outline" size="sm" onclick={() => openAdd('env')}>
						<Plus class="mr-2 h-3.5 w-3.5" />
						Add Variable
					</Button>
				</div>
			</div>

			{#if env.vars.length === 0}
				<div class="rounded border border-dashed border-border py-8 text-center">
					<p class="text-sm text-muted-foreground">No environment variables yet.</p>
					<p class="mt-1 text-xs text-muted-foreground">
						Add them one at a time, or paste a .env file with "Import .env".
					</p>
				</div>
			{:else}
				<EnvVarTable
					entries={env.vars}
					revealed={revealed.env}
					{busyKey}
					{copiedKey}
					noun="variable"
					excluded={previewEnabled ? excluded : undefined}
					onToggleExclude={previewEnabled ? handleToggleExclude : undefined}
					onToggleReveal={(key) => handleToggleReveal('env', key)}
					onCopy={(key) => handleCopy('env', key)}
					onEdit={(key) => openEdit('env', key)}
					onDelete={(key) => openDelete('env', key)}
				/>
			{/if}
		</section>

		{#if isImage}
			<Separator class="bg-border" />

			<section class="space-y-3">
				<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
					Registry Credentials
				</h4>
				<div class="space-y-2">
					<Select.Root
						type="single"
						value={selectedCredentialId}
						onValueChange={handleCredentialChange}
					>
						<Select.Trigger>
							{selectedCredentialId
								? (registryCredentials.find((c) => c.id === selectedCredentialId)?.name ??
									'Unknown')
								: 'None (public image)'}
						</Select.Trigger>
						<Select.Content>
							<Select.Item value="">None (public image)</Select.Item>
							{#each registryCredentials as cred (cred.id)}
								<Select.Item value={cred.id}>{cred.name} ({cred.server})</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
					<p class="text-xs text-muted-foreground">
						Required for private Docker registries. Saved as soon as you pick one.
					</p>
				</div>
			</section>
		{/if}

		{#if previewEnabled}
			<Separator class="bg-border" />

			<section class="space-y-3">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<div>
						<h4
							class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase"
						>
							Preview Overrides
						</h4>
						<p class="mt-0.5 text-xs text-muted-foreground">
							These values replace or add variables in preview deployments only.
						</p>
					</div>
					<div class="flex gap-2">
						<Button
							variant="outline"
							size="sm"
							onclick={() => ((importTarget = 'preview'), (importOpen = true))}
						>
							<FileUpload class="mr-2 h-3.5 w-3.5" />
							Import .env
						</Button>
						<Button variant="outline" size="sm" onclick={() => openAdd('preview')}>
							<Plus class="mr-2 h-3.5 w-3.5" />
							Add Override
						</Button>
					</div>
				</div>

				{#if env.preview.overrides.length === 0}
					<div class="rounded border border-dashed border-border py-6 text-center">
						<p class="text-sm text-muted-foreground">No preview overrides.</p>
						<p class="mt-1 text-xs text-muted-foreground">
							Previews get the app's variables, minus any you exclude above.
						</p>
					</div>
				{:else}
					<EnvVarTable
						entries={env.preview.overrides}
						revealed={revealed.preview}
						{busyKey}
						{copiedKey}
						noun="override"
						onToggleReveal={(key) => handleToggleReveal('preview', key)}
						onCopy={(key) => handleCopy('preview', key)}
						onEdit={(key) => openEdit('preview', key)}
						onDelete={(key) => openDelete('preview', key)}
					/>
				{/if}
			</section>
		{/if}
	{/if}
</div>

<EnvVarDialog
	bind:open={dialogOpen}
	{editingKey}
	existingKeys={dialogTarget === 'env' ? envKeys : overrideKeys}
	title={`${editingKey === null ? 'Add' : 'Edit'} ${dialogTarget === 'env' ? 'environment variable' : 'preview override'}`}
	description={dialogTarget === 'env'
		? 'Saved immediately. The app picks it up on the next deploy.'
		: 'Applies to preview deployments only, from their next deploy.'}
	onLoadCurrent={() => reveal(dialogTarget, editingKey as string)}
	onSubmit={handleDialogSubmit}
/>

<EnvImportDialog
	bind:open={importOpen}
	existingKeys={importTarget === 'env' ? envKeys : overrideKeys}
	targetLabel={importTarget === 'env' ? 'environment variables' : 'preview overrides'}
	onApply={(vars, redeploy) => handleImport(importTarget, vars, redeploy)}
/>

<Dialog.Root bind:open={deleteOpen}>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Delete {deleteKey}?</Dialog.Title>
			<Dialog.Description>
				{deleteTarget === 'env'
					? 'The variable is removed now. The running app keeps it until the next deploy.'
					: 'Preview deployments fall back to the app value (if any) from their next deploy.'}
			</Dialog.Description>
		</Dialog.Header>
		{#if deleteError}
			<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">
				{deleteError}
			</p>
		{/if}
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (deleteOpen = false)} disabled={deleting}>
				Cancel
			</Button>
			<Button variant="destructive" onclick={handleConfirmDelete} disabled={deleting}>
				{deleting ? 'Deleting…' : 'Delete'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
