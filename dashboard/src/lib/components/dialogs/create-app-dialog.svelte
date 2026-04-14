<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import { createApp, type App } from '$lib/api';

	interface Props {
		open: boolean;
		projectId?: string;
		oncreate?: (app: App) => void;
		oncancel?: () => void;
	}

	let { open = $bindable(), projectId, oncreate, oncancel }: Props = $props();

	let name = $state('');
	let sourceType = $state<'git' | 'image'>('git');
	let sourceUrl = $state('');
	let containerPort = $state(8080);
	let domain = $state('');
	let envVars = $state('');
	let error = $state('');
	let loading = $state(false);

	function reset() {
		name = '';
		sourceType = 'git';
		sourceUrl = '';
		containerPort = 8080;
		domain = '';
		envVars = '';
		error = '';
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';

		if (!name.trim()) {
			error = 'App name is required';
			return;
		}

		if (!sourceUrl.trim()) {
			error = 'Source URL is required';
			return;
		}

		loading = true;
		try {
			const env: Record<string, string> = {};
			for (const line of envVars.split('\n')) {
				const trimmed = line.trim();
				if (!trimmed || trimmed.startsWith('#')) continue;
				const eqIndex = trimmed.indexOf('=');
				if (eqIndex > 0) {
					env[trimmed.substring(0, eqIndex)] = trimmed.substring(eqIndex + 1);
				}
			}

			const app = await createApp({
				name: name.trim(),
				source_type: sourceType,
				source_url: sourceUrl.trim(),
				container_port: containerPort,
				...(domain.trim() ? { domain: domain.trim() } : {}),
				...(Object.keys(env).length > 0 ? { env_vars: env } : {}),
				...(projectId ? { project_id: projectId } : {})
			});

			reset();
			oncreate?.(app);
		} catch (e: any) {
			error = e.message ?? 'Failed to create app';
		} finally {
			loading = false;
		}
	}

	function handleOpenChange(isOpen: boolean) {
		if (!isOpen) {
			reset();
			oncancel?.();
		}
		open = isOpen;
	}
</script>

<Dialog.Root bind:open onOpenChange={handleOpenChange}>
	<Dialog.Content class="sm:max-w-150">
		<Dialog.Header>
			<Dialog.Title>Create New App</Dialog.Title>
			<Dialog.Description>
				Deploy a new application from a Git repository or Docker image.
			</Dialog.Description>
		</Dialog.Header>

		{#if error}
			<ErrorAlert title="Error" description={error} />
		{/if}

		<form onsubmit={handleSubmit} class="space-y-4">
			<div class="grid gap-4 sm:grid-cols-2">
				<div class="grid gap-2">
					<Label for="app-name">App Name *</Label>
					<Input
						id="app-name"
						type="text"
						bind:value={name}
						placeholder="my-awesome-app"
						required
					/>
				</div>

				<div class="grid gap-2">
					<Label for="source-type">Source Type</Label>
					<Select.Root type="single" bind:value={sourceType}>
						<Select.Trigger id="source-type">
							{sourceType === 'git' ? 'Git Repository' : 'Docker Image'}
						</Select.Trigger>
						<Select.Content>
							<Select.Item value="git">Git Repository</Select.Item>
							<Select.Item value="image">Docker Image</Select.Item>
						</Select.Content>
					</Select.Root>
				</div>
			</div>

			<div class="grid gap-2">
				<Label for="source-url">
					{sourceType === 'git' ? 'Git URL *' : 'Docker Image *'}
				</Label>
				<Input
					id="source-url"
					type="text"
					bind:value={sourceUrl}
					placeholder={sourceType === 'git'
						? 'https://github.com/org/repo.git'
						: 'registry.example.com/org/image:tag'}
					class="font-mono"
					required
				/>
				<p class="text-xs text-muted-foreground">
					{sourceType === 'git'
						? 'HTTPS or SSH format. For private repos, add a deploy key in app settings.'
						: 'Include registry if not Docker Hub. For private registries, add credentials in app settings.'}
				</p>
			</div>

			<div class="grid gap-4 sm:grid-cols-2">
				<div class="grid gap-2">
					<Label for="container-port">Container Port</Label>
					<Input id="container-port" type="number" bind:value={containerPort} min={1} max={65535} />
					<p class="text-xs text-muted-foreground">Port your app listens on inside the container</p>
				</div>

				<div class="grid gap-2">
					<Label for="domain">Domain (optional)</Label>
					<Input id="domain" type="text" bind:value={domain} placeholder="app.example.com" />
				</div>
			</div>

			<div class="grid gap-2">
				<Label for="env-vars">Environment Variables (optional)</Label>
				<Textarea
					id="env-vars"
					bind:value={envVars}
					rows={4}
					placeholder="DATABASE_URL=postgres://...&#10;SECRET_KEY=mysecret&#10;# Comments start with #"
					class="font-mono text-xs"
				/>
				<p class="text-xs text-muted-foreground">
					One KEY=value per line. Lines starting with # are ignored.
				</p>
			</div>

			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={() => handleOpenChange(false)}>
					Cancel
				</Button>
				<Button type="submit" disabled={loading}>
					{loading ? 'Creating...' : 'Create App'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
