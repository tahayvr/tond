<script lang="ts">
	import { Label } from '$lib/components/ui/label/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import { SvelteSet } from 'svelte/reactivity';
	import type { RegistryCredential } from '$lib/api';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';
	import Plus from '~icons/ri/add-line';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Pencil from '~icons/ri/pencil-line';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';
	import Copy from '~icons/ri/file-copy-line';
	import Check from '~icons/ri/check-line';

	interface Props {
		envVars: Record<string, string>;
		registryCredentialId: string;
		sourceSubType: SourceSubType;
		registryCredentials: RegistryCredential[];
	}

	let {
		envVars = $bindable({}),
		registryCredentialId = $bindable(''),
		sourceSubType,
		registryCredentials
	}: Props = $props();

	// Dialog state
	let dialogOpen = $state(false);
	/** null = adding new, string = editing existing key */
	let editingKey = $state<string | null>(null);
	let newKey = $state('');
	let newValue = $state('');
	let dialogError = $state('');

	// Per-row reveal / copy state
	let revealedKeys = new SvelteSet<string>();
	let copiedKey = $state<string | null>(null);
	let copyTimeout: ReturnType<typeof setTimeout> | null = null;

	let entries = $derived(Object.entries(envVars));

	function openAdd() {
		editingKey = null;
		newKey = '';
		newValue = '';
		dialogError = '';
		dialogOpen = true;
	}

	function openEdit(key: string, value: string) {
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
		if (editingKey !== null && editingKey !== key) {
			const next = { ...envVars };
			delete next[editingKey];
			revealedKeys.delete(editingKey);
			envVars = { ...next, [key]: newValue };
		} else {
			envVars = { ...envVars, [key]: newValue };
		}
		dialogOpen = false;
	}

	function handleRemove(key: string) {
		const next = { ...envVars };
		delete next[key];
		envVars = next;
		revealedKeys.delete(key);
	}

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

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') handleConfirm();
	}
</script>

<div class="space-y-6">
	<div class="space-y-3">
		<div class="flex items-center justify-between">
			<Label>Environment Variables</Label>
			<Dialog.Root bind:open={dialogOpen}>
				<Dialog.Trigger>
					{#snippet child({ props })}
						<Button variant="outline" size="sm" onclick={openAdd} {...props}>
							<Plus class="mr-2 h-3.5 w-3.5" />
							Add Variable
						</Button>
					{/snippet}
				</Dialog.Trigger>
				<Dialog.Content class="sm:max-w-md">
					<Dialog.Header>
						<Dialog.Title>{editingKey !== null ? 'Edit' : 'Add'} Environment Variable</Dialog.Title>
						<Dialog.Description>
							{editingKey !== null
								? 'Update the key or value for this environment variable.'
								: 'Enter a key and value for the new environment variable.'}
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
				<p class="text-sm text-muted-foreground">No environment variables added yet.</p>
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
							<th class="w-28 px-3 py-2"></th>
						</tr>
					</thead>
					<tbody>
						{#each entries as [key, value], i (key)}
							{@const revealed = revealedKeys.has(key)}
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
													<Check class="h-3.5 w-3.5 text-success" />
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
											onclick={() => openEdit(key, value)}
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

	{#if sourceSubType === 'image'}
		<Separator class="bg-border" />

		<div class="space-y-2">
			<Label>Registry Credentials</Label>
			<Select.Root type="single" bind:value={registryCredentialId}>
				<Select.Trigger>
					{registryCredentialId
						? (registryCredentials.find((c) => c.id === registryCredentialId)?.name ?? 'None')
						: 'None (public image)'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">None (public image)</Select.Item>
					{#each registryCredentials as credential (credential.id)}
						<Select.Item value={credential.id}>
							{credential.name} ({credential.server})
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">Required for private Docker registries</p>
		</div>
	{/if}
</div>
