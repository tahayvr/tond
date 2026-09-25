<script lang="ts">
	import {
		listStorageProviders,
		createStorageProvider,
		updateStorageProvider,
		deleteStorageProvider,
		testStorageProvider,
		type StorageProvider
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import Badge from '$lib/components/ui/badge/badge.svelte';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Plus from '~icons/ri/add-line';
	import Pencil from '~icons/ri/pencil-line';
	import FlaskConical from '~icons/ri/flask-line';

	// ── State ─────────────────────────────────────────────────────────────────

	let providers = $state<StorageProvider[]>([]);
	let loading = $state(true);
	let error = $state('');
	let deletingId = $state<string | null>(null);
	let testingId = $state<string | null>(null);
	let testMessage = $state('');
	let testOk = $state(false);

	// ── Add form ──────────────────────────────────────────────────────────────

	let showAdd = $state(false);
	let addName = $state('');
	let addEndpoint = $state('');
	let addRegion = $state('');
	let addBucket = $state('');
	let addPrefix = $state('');
	let addKeyId = $state('');
	let addSecret = $state('');
	let adding = $state(false);
	let addError = $state('');

	// ── Edit form ─────────────────────────────────────────────────────────────

	let editingId = $state<string | null>(null);
	let editName = $state('');
	let editEndpoint = $state('');
	let editRegion = $state('');
	let editBucket = $state('');
	let editPrefix = $state('');
	let editKeyId = $state('');
	let editSecret = $state(''); // blank means "don't change"
	let saving = $state(false);
	let editError = $state('');

	// ── Load ──────────────────────────────────────────────────────────────────

	async function load() {
		loading = true;
		error = '';
		try {
			providers = await listStorageProviders();
		} catch (e: any) {
			error = e.message ?? 'Failed to load storage providers';
		} finally {
			loading = false;
		}
	}

	// ── Add ───────────────────────────────────────────────────────────────────

	async function handleAdd() {
		addError = '';
		if (!addName.trim()) {
			addError = 'Name is required';
			return;
		}
		if (!addRegion.trim() || !addBucket.trim() || !addKeyId.trim() || !addSecret.trim()) {
			addError = 'Region, bucket, access key ID, and secret are required';
			return;
		}
		adding = true;
		try {
			const created = await createStorageProvider({
				name: addName.trim(),
				kind: 's3',
				config: {
					endpoint: addEndpoint.trim() || null,
					region: addRegion.trim(),
					bucket: addBucket.trim(),
					prefix: addPrefix.trim() || null,
					access_key_id: addKeyId.trim(),
					secret_access_key: addSecret
				}
			});
			providers = [created, ...providers];
			resetAddForm();
			showAdd = false;
		} catch (e: any) {
			addError = e.message ?? 'Failed to create storage provider';
		} finally {
			adding = false;
		}
	}

	function resetAddForm() {
		addName = '';
		addEndpoint = '';
		addRegion = '';
		addBucket = '';
		addPrefix = '';
		addKeyId = '';
		addSecret = '';
		addError = '';
	}

	function cancelAdd() {
		showAdd = false;
		resetAddForm();
	}

	function openAdd() {
		cancelEdit();
		showAdd = true;
	}

	// ── Edit ──────────────────────────────────────────────────────────────────

	function openEdit(p: StorageProvider) {
		cancelAdd();
		editingId = p.id;
		editName = p.name;
		editEndpoint = p.config.endpoint ?? '';
		editRegion = p.config.region;
		editBucket = p.config.bucket;
		editPrefix = p.config.prefix ?? '';
		editKeyId = p.config.access_key_id;
		editSecret = '';
		editError = '';
	}

	function cancelEdit() {
		editingId = null;
		editName = '';
		editEndpoint = '';
		editRegion = '';
		editBucket = '';
		editPrefix = '';
		editKeyId = '';
		editSecret = '';
		editError = '';
	}

	async function handleSave() {
		if (!editingId) return;
		editError = '';
		if (!editName.trim()) {
			editError = 'Name is required';
			return;
		}
		saving = true;
		const cfg: Record<string, unknown> = {
			endpoint: editEndpoint.trim() || null,
			region: editRegion.trim(),
			bucket: editBucket.trim(),
			prefix: editPrefix.trim() || null,
			access_key_id: editKeyId.trim()
		};
		if (editSecret) cfg['secret_access_key'] = editSecret;
		try {
			const updated = await updateStorageProvider(editingId, {
				name: editName.trim(),
				config: cfg as any
			});
			providers = providers.map((p) => (p.id === updated.id ? updated : p));
			cancelEdit();
		} catch (e: any) {
			editError = e.message ?? 'Failed to save changes';
		} finally {
			saving = false;
		}
	}

	// ── Delete ────────────────────────────────────────────────────────────────

	async function handleDelete(id: string) {
		deletingId = id;
		error = '';
		try {
			await deleteStorageProvider(id);
			providers = providers.filter((p) => p.id !== id);
			if (editingId === id) cancelEdit();
		} catch (e: any) {
			error = e.message ?? 'Failed to delete storage provider';
		} finally {
			deletingId = null;
		}
	}

	// ── Test ──────────────────────────────────────────────────────────────────

	async function handleTest(id: string) {
		testingId = id;
		testMessage = '';
		error = '';
		try {
			const result = await testStorageProvider(id);
			testOk = result.ok;
			testMessage = result.message;
		} catch (e: any) {
			testOk = false;
			testMessage = e.message ?? 'Connection test failed';
		} finally {
			testingId = null;
		}
	}

	function formatDate(dt: string) {
		return new Date(dt).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}

	load();
</script>

{#if error}
	<ErrorAlert title="Storage Provider Error" description={error} />
{/if}

{#if testMessage}
	<div
		class={[
			'rounded border p-3 text-xs',
			testOk
				? 'border-green-500 bg-green-500/10 text-green-600'
				: 'border-destructive bg-destructive/10 text-destructive'
		].join(' ')}
	>
		{testMessage}
	</div>
{/if}

<div class="space-y-3">
	<div class="flex items-center justify-between">
		<p class="text-sm text-muted-foreground">
			S3-compatible buckets used for offsite snapshot storage.
		</p>
		{#if !showAdd}
			<Button size="sm" class="h-7 text-xs uppercase" onclick={openAdd}>
				<Plus class="h-3 w-3" /> Add Provider
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
			<p class="text-xs font-semibold uppercase">New Storage Provider</p>
			{#if addError}
				<p class="text-xs text-destructive">{addError}</p>
			{/if}
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-name">Display Name</Label>
					<Input
						id="sp-name"
						class="h-8 text-xs"
						bind:value={addName}
						placeholder="e.g. Cloudflare R2 (prod)"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-endpoint"
						>Endpoint URL <span class="text-muted-foreground">(optional for AWS S3)</span></Label
					>
					<Input
						id="sp-endpoint"
						class="h-8 text-xs"
						bind:value={addEndpoint}
						placeholder="https://account.r2.cloudflarestorage.com"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-region">Region</Label>
					<Input
						id="sp-region"
						class="h-8 text-xs"
						bind:value={addRegion}
						placeholder="us-east-1"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-bucket">Bucket</Label>
					<Input
						id="sp-bucket"
						class="h-8 text-xs"
						bind:value={addBucket}
						placeholder="my-backups"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-prefix"
						>Key Prefix <span class="text-muted-foreground">(optional)</span></Label
					>
					<Input id="sp-prefix" class="h-8 text-xs" bind:value={addPrefix} placeholder="tond/" />
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="sp-keyid">Access Key ID</Label>
					<Input
						id="sp-keyid"
						class="h-8 text-xs"
						bind:value={addKeyId}
						placeholder="AKIAIOSFODNN7EXAMPLE"
					/>
				</div>
				<div class="grid gap-1.5 sm:col-span-2">
					<Label class="text-xs" for="sp-secret">Secret Access Key</Label>
					<Input
						id="sp-secret"
						type="password"
						class="h-8 text-xs"
						bind:value={addSecret}
						placeholder="••••••••"
					/>
				</div>
			</div>
			<div class="flex gap-2 pt-1">
				<Button type="submit" size="sm" class="h-8 text-xs uppercase" disabled={adding}>
					{adding ? 'Saving…' : 'Save Provider'}
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
	{:else if providers.length === 0 && !showAdd}
		<div class="border border-dashed py-10 text-center text-xs text-muted-foreground">
			No storage providers configured yet. Add one to enable offsite snapshot uploads.
		</div>
	{:else}
		<div class="space-y-px">
			{#each providers as provider (provider.id)}
				<div class="border bg-background text-xs">
					<div class="flex items-center justify-between px-4 py-3">
						<div class="flex min-w-0 flex-col gap-0.5">
							<div class="flex items-center gap-2">
								<span class="font-medium">{provider.name}</span>
								<Badge variant="outline" class="h-4 px-1 text-[10px] uppercase">
									{provider.kind}
								</Badge>
							</div>
							<span class="text-muted-foreground">
								{provider.config.bucket}{provider.config.prefix ? `/${provider.config.prefix}` : ''} ·
								Added {formatDate(provider.created_at)}
							</span>
						</div>
						<div class="ml-4 flex shrink-0 items-center gap-1">
							<Button
								variant="ghost"
								size="sm"
								class="h-7 text-xs uppercase"
								disabled={testingId === provider.id}
								onclick={() => handleTest(provider.id)}
							>
								<FlaskConical class="h-3.5 w-3.5" />
								{testingId === provider.id ? 'Testing…' : 'Test'}
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7"
								aria-label="Edit"
								disabled={editingId === provider.id}
								onclick={() => openEdit(provider)}
							>
								<Pencil class="h-3.5 w-3.5" />
							</Button>
							<Button
								variant="ghost"
								size="icon"
								class="h-7 w-7 text-destructive hover:text-destructive"
								disabled={deletingId === provider.id}
								onclick={() => handleDelete(provider.id)}
							>
								<Trash2 class="h-3.5 w-3.5" />
							</Button>
						</div>
					</div>

					{#if editingId === provider.id}
						<form
							onsubmit={(e) => {
								e.preventDefault();
								handleSave();
							}}
							class="space-y-3 border-t bg-muted/30 px-4 py-4"
						>
							<p class="text-xs font-semibold uppercase">Edit Storage Provider</p>
							{#if editError}
								<p class="text-xs text-destructive">{editError}</p>
							{/if}
							<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-name">Display Name</Label>
									<Input id="edit-sp-name" class="h-8 text-xs" bind:value={editName} />
								</div>
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-endpoint">Endpoint URL</Label>
									<Input
										id="edit-sp-endpoint"
										class="h-8 text-xs"
										bind:value={editEndpoint}
										placeholder="https://…"
									/>
								</div>
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-region">Region</Label>
									<Input id="edit-sp-region" class="h-8 text-xs" bind:value={editRegion} />
								</div>
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-bucket">Bucket</Label>
									<Input id="edit-sp-bucket" class="h-8 text-xs" bind:value={editBucket} />
								</div>
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-prefix">Key Prefix</Label>
									<Input
										id="edit-sp-prefix"
										class="h-8 text-xs"
										bind:value={editPrefix}
										placeholder="tond/"
									/>
								</div>
								<div class="grid gap-1.5">
									<Label class="text-xs" for="edit-sp-keyid">Access Key ID</Label>
									<Input id="edit-sp-keyid" class="h-8 text-xs" bind:value={editKeyId} />
								</div>
								<div class="grid gap-1.5 sm:col-span-2">
									<Label class="text-xs" for="edit-sp-secret">
										Secret Access Key
										<span class="text-muted-foreground">(leave blank to keep existing)</span>
									</Label>
									<Input
										id="edit-sp-secret"
										type="password"
										class="h-8 text-xs"
										bind:value={editSecret}
										placeholder="••••••••"
									/>
								</div>
							</div>
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
