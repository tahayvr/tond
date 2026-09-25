<script lang="ts">
	import { Label } from '$lib/components/ui/label/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { SvelteSet } from 'svelte/reactivity';
	import { untrack } from 'svelte';
	import type { RegistryCredential } from '$lib/api';
	import Plus from '~icons/ri/add-line';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Pencil from '~icons/ri/pencil-line';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';
	import Copy from '~icons/ri/file-copy-line';
	import Check from '~icons/ri/check-line';

	interface Props {
		/** Source type of the app — registry section only shown for image sources */
		sourceType: string;
		registryCredentials: RegistryCredential[];
		registryCredentialId: string | null;
		saving: boolean;
		saveError: string;
		/** Whether PR preview deployments are enabled for this app */
		previewEnabled: boolean;
		/** Keys excluded from preview containers */
		previewExcludedKeys: string[];
		/** Override env vars for preview containers */
		previewEnvVars: Record<string, string>;
		onSave: (
			envVars: Record<string, string>,
			credentialId: string | null,
			previewExcludedKeys: string[],
			previewEnvVars: Record<string, string>
		) => void;
	}

	let {
		sourceType,
		registryCredentials,
		registryCredentialId,
		saving,
		saveError,
		previewEnabled,
		previewExcludedKeys,
		previewEnvVars,
		onSave
	}: Props = $props();

	// Local mutable state — values are write-only from API, so we maintain them locally.
	let envVars = $state<Record<string, string>>({});
	let selectedCredentialId: string | undefined = $derived(registryCredentialId ?? undefined);

	// Scoped secrets state — initialised from props
	let excludedKeys = new SvelteSet<string>(untrack(() => previewExcludedKeys));
	let previewOverrides = $state<Record<string, string>>(untrack(() => ({ ...previewEnvVars })));

	// Dialog state (shared for both env vars and preview overrides)
	let dialogOpen = $state(false);
	let dialogTarget = $state<'env' | 'preview'>('env');
	/** null = adding new, string = editing existing key */
	let editingKey = $state<string | null>(null);
	let newKey = $state('');
	let newValue = $state('');
	let dialogError = $state('');

	let entries = $derived(Object.entries(envVars));
	let previewOverrideEntries = $derived(Object.entries(previewOverrides));
	let isImage = $derived(sourceType === 'image');

	// Per-row reveal and copy state, keyed by env var key
	let revealedKeys = new SvelteSet<string>();
	let copiedKey = $state<string | null>(null);
	let copyTimeout: ReturnType<typeof setTimeout> | null = null;

	function toggleReveal(key: string) {
		if (revealedKeys.has(key)) {
			revealedKeys.delete(key);
		} else {
			revealedKeys.add(key);
		}
	}

	async function handleCopy(key: string, value: string) {
		await navigator.clipboard.writeText(value);
		copiedKey = key;
		if (copyTimeout) clearTimeout(copyTimeout);
		copyTimeout = setTimeout(() => {
			copiedKey = null;
		}, 2000);
	}

	function openAdd(target: 'env' | 'preview' = 'env') {
		dialogTarget = target;
		editingKey = null;
		newKey = '';
		newValue = '';
		dialogError = '';
		dialogOpen = true;
	}

	function openEdit(target: 'env' | 'preview', key: string, value: string) {
		dialogTarget = target;
		editingKey = key;
		newKey = key;
		newValue = value;
		dialogError = '';
		dialogOpen = true;
	}

	function handleConfirm() {
		const key = newKey.trim();
		if (!key) {
			dialogError = 'Key is required';
			return;
		}
		if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
			dialogError =
				'Key must start with a letter or underscore and contain only letters, numbers, and underscores';
			return;
		}

		if (dialogTarget === 'env') {
			// If renaming (key changed), remove the old key first
			if (editingKey !== null && editingKey !== key) {
				const next = { ...envVars };
				delete next[editingKey];
				envVars = { ...next, [key]: newValue };
				// Move exclusion if the old key was excluded
				if (excludedKeys.has(editingKey)) {
					excludedKeys.delete(editingKey);
					excludedKeys.add(key);
				}
			} else {
				envVars = { ...envVars, [key]: newValue };
			}
		} else {
			if (editingKey !== null && editingKey !== key) {
				const next = { ...previewOverrides };
				delete next[editingKey];
				previewOverrides = { ...next, [key]: newValue };
			} else {
				previewOverrides = { ...previewOverrides, [key]: newValue };
			}
		}

		dialogOpen = false;
	}

	function handleRemove(key: string) {
		const next = { ...envVars };
		delete next[key];
		envVars = next;
		revealedKeys.delete(key);
		excludedKeys.delete(key);
	}

	function handleRemoveOverride(key: string) {
		const next = { ...previewOverrides };
		delete next[key];
		previewOverrides = next;
	}

	function toggleExclude(key: string) {
		if (excludedKeys.has(key)) {
			excludedKeys.delete(key);
		} else {
			excludedKeys.add(key);
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') handleConfirm();
	}

	function handleSave() {
		onSave(envVars, selectedCredentialId || null, [...excludedKeys], previewOverrides);
	}
</script>

<div class="space-y-6 pt-4">
	<!-- Variables section -->
	<div class="space-y-3">
		<div class="flex items-center justify-between">
			<div>
				<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
					Environment Variables
				</h4>
				{#if previewEnabled}
					<p class="mt-0.5 text-xs text-muted-foreground">
						Check "Exclude" to hide a variable from preview containers.
					</p>
				{/if}
			</div>
			<Dialog.Root bind:open={dialogOpen}>
				<Dialog.Trigger>
					{#snippet child({ props })}
						<Button variant="outline" size="sm" onclick={() => openAdd('env')} {...props}>
							<Plus class="mr-2 h-3.5 w-3.5" />
							Add Variable
						</Button>
					{/snippet}
				</Dialog.Trigger>
				<Dialog.Content class="sm:max-w-md">
					<Dialog.Header>
						<Dialog.Title>
							{editingKey !== null ? 'Edit' : 'Add'}
							{dialogTarget === 'preview' ? 'Preview Override' : 'Environment Variable'}
						</Dialog.Title>
						<Dialog.Description>
							{#if dialogTarget === 'preview'}
								{editingKey !== null
									? 'Update the key or value for this preview override.'
									: 'Enter a key and value to override in preview containers.'}
							{:else}
								{editingKey !== null
									? 'Update the key or value for this environment variable.'
									: 'Enter a key and value for the new environment variable.'}
							{/if}
						</Dialog.Description>
					</Dialog.Header>
					<div class="space-y-4 py-2">
						<div class="space-y-2">
							<Label for="env-key">Key</Label>
							<Input
								id="env-key"
								bind:value={newKey}
								placeholder="DATABASE_URL"
								class="font-mono"
								onkeydown={handleKeydown}
								autofocus
							/>
						</div>
						<div class="space-y-2">
							<Label for="env-value">Value</Label>
							<Input
								id="env-value"
								bind:value={newValue}
								placeholder="postgres://user:pass@host/db"
								class="font-mono"
								onkeydown={handleKeydown}
							/>
						</div>
						{#if dialogError}
							<p class="text-xs text-destructive">{dialogError}</p>
						{/if}
					</div>
					<Dialog.Footer>
						<Button variant="outline" onclick={() => (dialogOpen = false)}>Cancel</Button>
						<Button onclick={handleConfirm}>{editingKey !== null ? 'Save' : 'Add'}</Button>
					</Dialog.Footer>
				</Dialog.Content>
			</Dialog.Root>
		</div>

		{#if entries.length === 0}
			<div class="rounded border border-dashed border-border py-8 text-center">
				<p class="text-sm text-muted-foreground">No environment variables defined.</p>
				<p class="mt-1 text-xs text-muted-foreground">
					Click "Add Variable" to define key-value pairs.
				</p>
			</div>
		{:else}
			<div class="overflow-hidden rounded border border-border">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-border bg-muted/50">
							<th
								class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
								>Key</th
							>
							<th
								class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
								>Value</th
							>
							{#if previewEnabled}
								<th
									class="w-20 px-3 py-2 text-center font-mono text-xs font-medium text-muted-foreground uppercase"
									>Exclude</th
								>
							{/if}
							<th class="w-28 px-3 py-2"></th>
						</tr>
					</thead>
					<tbody>
						{#each entries as [key, value], i (key)}
							{@const revealed = revealedKeys.has(key)}
							{@const excluded = excludedKeys.has(key)}
							<tr class={i < entries.length - 1 ? 'border-b border-border' : ''}>
								<td class="px-3 py-2 font-mono text-xs font-medium">{key}</td>
								<td class="max-w-0 truncate px-3 py-2 font-mono text-xs text-muted-foreground">
									{#if !value}
										<span class="italic">empty</span>
									{:else if revealed}
										{value}
									{:else}
										<span class="tracking-widest">••••••••</span>
									{/if}
								</td>
								{#if previewEnabled}
									<td class="px-3 py-2 text-center">
										<input
											type="checkbox"
											checked={excluded}
											onchange={() => toggleExclude(key)}
											class="h-4 w-4 cursor-pointer accent-primary"
											aria-label="Exclude from previews"
										/>
									</td>
								{/if}
								<td class="px-3 py-2">
									<div class="flex items-center justify-end gap-1">
										{#if value}
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7 text-muted-foreground hover:text-foreground"
												aria-label={revealed ? 'Hide value' : 'Reveal value'}
												onclick={() => toggleReveal(key)}
											>
												{#if revealed}
													<EyeOff class="h-3.5 w-3.5" />
												{:else}
													<Eye class="h-3.5 w-3.5" />
												{/if}
											</Button>
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7 text-muted-foreground hover:text-foreground"
												aria-label="Copy value"
												onclick={() => handleCopy(key, value)}
											>
												{#if copiedKey === key}
													<Check class="h-3.5 w-3.5 text-green-500" />
												{:else}
													<Copy class="h-3.5 w-3.5" />
												{/if}
											</Button>
										{/if}
										<Button
											variant="ghost"
											size="icon"
											class="h-7 w-7 text-muted-foreground hover:text-foreground"
											aria-label="Edit variable"
											onclick={() => openEdit('env', key, value)}
										>
											<Pencil class="h-3.5 w-3.5" />
										</Button>
										<Button
											variant="ghost"
											size="icon"
											class="h-7 w-7 text-muted-foreground hover:text-destructive"
											aria-label="Delete variable"
											onclick={() => handleRemove(key)}
										>
											<Trash2 class="h-3.5 w-3.5" />
										</Button>
									</div>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>

	{#if isImage}
		<Separator class="bg-border" />

		<!-- Registry credentials section -->
		<div class="space-y-3">
			<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
				Registry Credentials
			</h4>
			<div class="space-y-2">
				<Select.Root type="single" bind:value={selectedCredentialId}>
					<Select.Trigger>
						{selectedCredentialId
							? (registryCredentials.find((c) => c.id === selectedCredentialId)?.name ?? 'Unknown')
							: 'None (public image)'}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="">None (public image)</Select.Item>
						{#each registryCredentials as cred (cred.id)}
							<Select.Item value={cred.id}>{cred.name} ({cred.server})</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<p class="text-xs text-muted-foreground">Required for private Docker registries.</p>
			</div>
		</div>
	{/if}

	{#if previewEnabled}
		<Separator class="bg-border" />

		<!-- Preview overrides section -->
		<div class="space-y-3">
			<div class="flex items-center justify-between">
				<div>
					<h4 class="font-mono text-xs font-medium tracking-wider text-muted-foreground uppercase">
						Preview Overrides
					</h4>
					<p class="mt-0.5 text-xs text-muted-foreground">
						These key-value pairs replace or add env vars in preview containers only.
					</p>
				</div>
				<Button variant="outline" size="sm" onclick={() => openAdd('preview')}>
					<Plus class="mr-2 h-3.5 w-3.5" />
					Add Override
				</Button>
			</div>

			{#if previewOverrideEntries.length === 0}
				<div class="rounded border border-dashed border-border py-6 text-center">
					<p class="text-sm text-muted-foreground">No preview overrides defined.</p>
					<p class="mt-1 text-xs text-muted-foreground">
						Overrides replace or add env vars in preview containers only.
					</p>
				</div>
			{:else}
				<div class="overflow-hidden rounded border border-border">
					<table class="w-full text-sm">
						<thead>
							<tr class="border-b border-border bg-muted/50">
								<th
									class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
									>Key</th
								>
								<th
									class="px-3 py-2 text-left font-mono text-xs font-medium text-muted-foreground uppercase"
									>Value</th
								>
								<th class="w-20 px-3 py-2"></th>
							</tr>
						</thead>
						<tbody>
							{#each previewOverrideEntries as [key, value], i (key)}
								<tr class={i < previewOverrideEntries.length - 1 ? 'border-b border-border' : ''}>
									<td class="px-3 py-2 font-mono text-xs font-medium">{key}</td>
									<td class="max-w-0 truncate px-3 py-2 font-mono text-xs text-muted-foreground">
										{#if value}
											{value}
										{:else}
											<span class="italic">empty</span>
										{/if}
									</td>
									<td class="px-3 py-2">
										<div class="flex items-center justify-end gap-1">
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7 text-muted-foreground hover:text-foreground"
												aria-label="Edit override"
												onclick={() => openEdit('preview', key, value)}
											>
												<Pencil class="h-3.5 w-3.5" />
											</Button>
											<Button
												variant="ghost"
												size="icon"
												class="h-7 w-7 text-muted-foreground hover:text-destructive"
												aria-label="Delete override"
												onclick={() => handleRemoveOverride(key)}
											>
												<Trash2 class="h-3.5 w-3.5" />
											</Button>
										</div>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>
	{/if}

	<Separator class="bg-border" />

	<!-- Save footer -->
	<div class="flex items-center gap-4">
		<Button onclick={handleSave} disabled={saving}>
			{saving ? 'Saving…' : 'Save'}
		</Button>
		<p class="text-xs text-muted-foreground">Changes take effect after redeploying.</p>
		{#if saveError}
			<p class="text-xs text-destructive">{saveError}</p>
		{/if}
	</div>
</div>
