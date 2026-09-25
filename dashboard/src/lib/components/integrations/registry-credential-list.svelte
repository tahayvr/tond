<script lang="ts">
	import {
		listRegistryCredentials,
		createRegistryCredential,
		deleteRegistryCredential,
		type RegistryCredential
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Plus from '~icons/ri/add-line';

	interface Props {
		initialCredentials?: RegistryCredential[];
	}

	let { initialCredentials }: Props = $props();
	let initialized = $state(false);

	const REGISTRY_PRESETS = [
		{ label: 'Docker Hub', value: 'registry-1.docker.io' },
		{ label: 'GHCR', value: 'ghcr.io' },
		{ label: 'GitLab', value: 'registry.gitlab.com' }
	];

	// ── State ─────────────────────────────────────────────────────────────────

	let credentials = $state<RegistryCredential[]>([]);
	let loading = $state(true);
	let error = $state('');
	let deletingId = $state<string | null>(null);

	// ── Add form ──────────────────────────────────────────────────────────────

	let showAdd = $state(false);
	let addName = $state('');
	let addServer = $state('');
	let addUsername = $state('');
	let addPassword = $state('');
	let adding = $state(false);
	let addError = $state('');

	// ── Load ──────────────────────────────────────────────────────────────────

	async function load() {
		loading = true;
		error = '';
		try {
			credentials = await listRegistryCredentials();
		} catch (e: any) {
			error = e.message ?? 'Failed to load registry credentials';
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
		if (!addServer.trim()) {
			addError = 'Registry server is required';
			return;
		}
		if (!addUsername.trim()) {
			addError = 'Username is required';
			return;
		}
		if (!addPassword.trim()) {
			addError = 'Password / token is required';
			return;
		}

		adding = true;
		try {
			const cred = await createRegistryCredential({
				name: addName.trim(),
				server: addServer.trim(),
				username: addUsername.trim(),
				password: addPassword.trim()
			});
			credentials = [...credentials, cred];
			addName = '';
			addServer = '';
			addUsername = '';
			addPassword = '';
			showAdd = false;
		} catch (e: any) {
			addError = e.message ?? 'Failed to add credential';
		} finally {
			adding = false;
		}
	}

	async function handleDelete(id: string) {
		deletingId = id;
		error = '';
		try {
			await deleteRegistryCredential(id);
			credentials = credentials.filter((c) => c.id !== id);
		} catch (e: any) {
			error = e.message ?? 'Failed to remove credential';
		} finally {
			deletingId = null;
		}
	}

	function cancelAdd() {
		showAdd = false;
		addError = '';
		addName = '';
		addServer = '';
		addUsername = '';
		addPassword = '';
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

		if (initialCredentials !== undefined) {
			credentials = initialCredentials;
			loading = false;
			return;
		}

		void load();
	});
</script>

{#if error}
	<ErrorAlert title="Registry Error" description={error} />
{/if}

<div class="space-y-3">
	<div class="flex items-center justify-between">
		<p class="text-sm text-muted-foreground">
			Credentials for private container registries, used at deploy time.
		</p>
		{#if !showAdd}
			<Button size="sm" class="h-7 text-xs uppercase" onclick={() => (showAdd = true)}>
				<Plus class="h-3 w-3" /> Add Credential
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
			<p class="text-xs font-semibold uppercase">New Credential</p>
			{#if addError}
				<p class="text-xs text-destructive">{addError}</p>
			{/if}
			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div class="grid gap-1.5">
					<Label class="text-xs" for="cred-name">Name</Label>
					<Input
						id="cred-name"
						class="h-8 text-xs"
						bind:value={addName}
						placeholder="GitHub Container Registry"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="cred-server">Registry Server</Label>
					<Input
						id="cred-server"
						class="h-8 text-xs"
						bind:value={addServer}
						placeholder="ghcr.io"
					/>
					<div class="flex flex-wrap gap-1">
						{#each REGISTRY_PRESETS as preset (preset.value)}
							<button
								type="button"
								onclick={() => (addServer = preset.value)}
								class="rounded border px-1.5 py-0.5 text-[10px] text-muted-foreground hover:bg-muted hover:text-foreground"
							>
								{preset.label}
							</button>
						{/each}
					</div>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="cred-username">Username</Label>
					<Input
						id="cred-username"
						class="h-8 text-xs"
						bind:value={addUsername}
						placeholder="myorg"
					/>
				</div>
				<div class="grid gap-1.5">
					<Label class="text-xs" for="cred-password">Password / Token</Label>
					<Input
						id="cred-password"
						type="password"
						class="h-8 text-xs"
						bind:value={addPassword}
						placeholder="ghp_xxxxxxxxxxxxxxxxxxxx"
					/>
				</div>
			</div>
			<div class="flex gap-2 pt-1">
				<Button type="submit" size="sm" class="h-8 text-xs uppercase" disabled={adding}>
					{adding ? 'Saving…' : 'Save Credential'}
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
	{:else if credentials.length === 0 && !showAdd}
		<div class="border border-dashed py-10 text-center text-xs text-muted-foreground">
			No registry credentials yet. Add one to pull from private registries.
		</div>
	{:else}
		<div class="space-y-px">
			{#each credentials as cred (cred.id)}
				<div class="flex items-center justify-between border bg-background px-4 py-3 text-xs">
					<div class="flex min-w-0 flex-col gap-0.5">
						<span class="font-medium">{cred.name}</span>
						<span class="text-muted-foreground">
							{cred.server} · {cred.username} · Added {formatDate(cred.created_at)}
						</span>
					</div>
					<Button
						variant="ghost"
						size="icon"
						class="ml-4 h-7 w-7 shrink-0 text-destructive hover:text-destructive"
						aria-label={`Delete ${cred.name}`}
						disabled={deletingId === cred.id}
						onclick={() => handleDelete(cred.id)}
					>
						<Trash2 class="h-3.5 w-3.5" />
					</Button>
				</div>
			{/each}
		</div>
	{/if}
</div>
