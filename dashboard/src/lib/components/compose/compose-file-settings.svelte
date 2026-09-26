<script lang="ts">
	// Settings → Compose file: edit the inline file (checked as you type) or the
	// repository it's read from, then save or save and deploy.
	import { updateCompose, type App } from '$lib/api';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as ButtonGroup from '$lib/components/ui/button-group/index.js';
	import ComposeEditor from './compose-editor.svelte';

	interface Props {
		app: App;
		/** Editing compose files needs settings:write (it reaches the host). */
		canEdit: boolean;
		canDeploy: boolean;
		onSaved: () => unknown;
		onDeploy: () => unknown;
	}

	let { app, canEdit, canDeploy, onSaved, onDeploy }: Props = $props();

	let mode = $state<'inline' | 'git'>('inline');
	let content = $state('');
	let sourceUrl = $state('');
	let branch = $state('');
	let composePath = $state('');
	let problems = $state<string[]>([]);
	let saving = $state(false);
	let error = $state('');
	let saved = $state('');

	// Reset the form from the app (on load and after a save).
	$effect(() => {
		mode = app.source_url ? 'git' : 'inline';
		content = app.compose_content ?? '';
		sourceUrl = app.source_url ?? '';
		branch = app.branch ?? '';
		composePath = app.compose_path ?? 'docker-compose.yml';
	});

	let dirty = $derived(
		mode === 'inline'
			? !!app.source_url || content !== (app.compose_content ?? '')
			: sourceUrl !== (app.source_url ?? '') ||
					composePath !== (app.compose_path ?? 'docker-compose.yml') ||
					branch !== (app.branch ?? '')
	);

	async function save(deployAfter: boolean) {
		error = '';
		saved = '';
		if (mode === 'inline' && !content.trim()) {
			error = 'The compose file is empty.';
			return;
		}
		if (mode === 'inline' && problems.length) {
			error = 'Fix the problems listed under the editor first.';
			return;
		}
		if (mode === 'git' && !sourceUrl.trim()) {
			error = 'Enter the repository URL.';
			return;
		}
		saving = true;
		try {
			const result = await updateCompose(
				app.id,
				mode === 'inline'
					? { compose_content: content }
					: {
							source_url: sourceUrl.trim(),
							branch: branch.trim() || undefined,
							compose_path: composePath.trim() || undefined
						}
			);
			await onSaved();
			if (deployAfter) {
				await onDeploy();
			} else {
				saved = result.message;
			}
		} catch (e: any) {
			error = e.message ?? 'Could not save the compose file';
		} finally {
			saving = false;
		}
	}
</script>

<div class="space-y-4 pt-4">
	<div class="space-y-1">
		<h2 class="text-base font-semibold">Compose file</h2>
		<p class="text-sm text-muted-foreground">
			Changes take effect on the next deploy. Every deployment keeps its compose file and images, so
			you can roll back from Deployments.
		</p>
	</div>

	{#if !canEdit}
		<p class="rounded-sm border border-dashed px-3 py-2 text-sm text-muted-foreground">
			Editing compose files needs the <code>settings:write</code> permission (an admin), because a compose
			file reaches the server.
		</p>
	{/if}

	<nav aria-label="Where the compose file comes from">
		<ButtonGroup.Root>
			<Button
				size="sm"
				variant={mode === 'inline' ? 'default' : 'outline'}
				aria-pressed={mode === 'inline'}
				onclick={() => (mode = 'inline')}
				disabled={!canEdit}>Paste a file</Button
			>
			<Button
				size="sm"
				variant={mode === 'git' ? 'default' : 'outline'}
				aria-pressed={mode === 'git'}
				onclick={() => (mode = 'git')}
				disabled={!canEdit}>From a Git repository</Button
			>
		</ButtonGroup.Root>
	</nav>

	{#if mode === 'inline'}
		<ComposeEditor
			bind:content
			appName={app.name}
			id="compose-settings-content"
			disabled={!canEdit || saving}
			onChecked={(errors) => (problems = errors)}
		/>
	{:else}
		<div class="grid gap-4 sm:grid-cols-2">
			<div class="space-y-1.5 sm:col-span-2">
				<Label for="compose-repo">Repository URL</Label>
				<Input
					id="compose-repo"
					class="font-mono"
					placeholder="https://github.com/acme/shop.git"
					bind:value={sourceUrl}
					disabled={!canEdit || saving}
				/>
			</div>
			<div class="space-y-1.5">
				<Label for="compose-branch">Branch</Label>
				<Input
					id="compose-branch"
					class="font-mono"
					placeholder="main"
					bind:value={branch}
					disabled={!canEdit || saving}
				/>
			</div>
			<div class="space-y-1.5">
				<Label for="compose-path">Compose file path</Label>
				<Input
					id="compose-path"
					class="font-mono"
					placeholder="docker-compose.yml"
					bind:value={composePath}
					disabled={!canEdit || saving}
				/>
			</div>
			<p class="text-xs text-muted-foreground sm:col-span-2">
				Tond clones the branch on every deploy, reads the compose file and builds services with a
				<code class="font-mono">build:</code> section from the repository. The file is checked against
				the same rules as a pasted one.
			</p>
		</div>
	{/if}

	<InlineError message={error} ondismiss={() => (error = '')} />
	<SuccessAlert message={saved} ondismiss={() => (saved = '')} />

	{#if canEdit}
		<div class="flex flex-wrap gap-2">
			<Button onclick={() => save(false)} disabled={saving || !dirty} aria-busy={saving}>
				{saving ? 'Saving…' : 'Save'}
			</Button>
			{#if canDeploy}
				<Button variant="outline" onclick={() => save(true)} disabled={saving || !dirty}>
					Save and deploy
				</Button>
			{/if}
		</div>
	{/if}
</div>
