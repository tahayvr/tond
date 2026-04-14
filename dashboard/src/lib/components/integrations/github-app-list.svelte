<script lang="ts">
	import {
		listGitHubApps,
		getGitHubAppManifestUrl,
		deleteGitHubApp,
		type GitHubApp
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import GitHubAppCard from './github-app-card.svelte';
	import Plus from '@lucide/svelte/icons/plus';

	interface Props {
		initialApps?: GitHubApp[];
	}

	let { initialApps }: Props = $props();
	let initialized = $state(false);

	// ── State ─────────────────────────────────────────────────────────────────

	let apps = $state<GitHubApp[]>([]);
	let loading = $state(true);
	let connecting = $state(false);
	let disconnectingId = $state<string | null>(null);
	let error = $state('');

	// ── Load ──────────────────────────────────────────────────────────────────

	export async function load() {
		loading = true;
		error = '';
		try {
			apps = await listGitHubApps();
		} catch (e: any) {
			error = e.message ?? 'Failed to load GitHub Apps';
		} finally {
			loading = false;
		}
	}

	// ── Actions ───────────────────────────────────────────────────────────────

	async function handleConnect() {
		connecting = true;
		error = '';
		try {
			const { github_url, manifest } = await getGitHubAppManifestUrl();
			// GitHub's Manifest flow requires a POST form submission — a GET redirect
			// with the manifest in the query string lands on a blank "Create New App"
			// form instead of the pre-configured one.
			const form = document.createElement('form');
			form.method = 'POST';
			form.action = github_url;
			const input = document.createElement('input');
			input.type = 'hidden';
			input.name = 'manifest';
			input.value = manifest;
			form.appendChild(input);
			document.body.appendChild(form);
			form.submit();
		} catch (e: any) {
			error = e.message ?? 'Failed to start GitHub App connection';
			connecting = false;
		}
	}

	async function handleDisconnect(id: string) {
		disconnectingId = id;
		error = '';
		try {
			await deleteGitHubApp(id);
			apps = apps.filter((a) => a.id !== id);
		} catch (e: any) {
			error = e.message ?? 'Failed to disconnect GitHub App';
		} finally {
			disconnectingId = null;
		}
	}

	function handleInstall(app: GitHubApp) {
		window.location.href = app.install_url;
	}

	function handleRename(updated: GitHubApp) {
		apps = apps.map((a) => (a.id === updated.id ? updated : a));
	}

	// ── Silent refresh (called by parent on window focus) ─────────────────────

	export async function silentRefresh() {
		try {
			apps = await listGitHubApps();
		} catch {
			// Silently ignore
		}
	}

	$effect(() => {
		if (initialized) return;
		initialized = true;

		if (initialApps !== undefined) {
			apps = initialApps;
			loading = false;
			return;
		}

		void load();
	});
</script>

{#if error}
	<ErrorAlert title="GitHub Error" description={error} />
{/if}

{#if loading}
	<p class="text-xs text-muted-foreground">Loading…</p>
{:else if apps.length === 0}
	<!-- Step 1: no app connected yet -->
	<div class="space-y-6">
		<div class="space-y-2">
			<p class="text-sm font-medium">Step 1 of 2 — Create GitHub App</p>
			<p class="text-sm text-muted-foreground">
				Connect a GitHub App to enable private repository access and automatic push-triggered
				deployments. You'll be redirected to GitHub to create the app in one click.
			</p>
		</div>
		<Button disabled={connecting} onclick={handleConnect} class="uppercase">
			{connecting ? 'Redirecting to GitHub…' : 'Create GitHub App'}
		</Button>
	</div>
{:else}
	<div class="space-y-4">
		{#each apps as app (app.id)}
			<GitHubAppCard
				{app}
				disconnecting={disconnectingId === app.id}
				onDisconnect={() => handleDisconnect(app.id)}
				onInstall={() => handleInstall(app)}
				onRename={handleRename}
			/>
		{/each}

		<div>
			<Button
				variant="outline"
				size="sm"
				class="h-7 text-xs uppercase"
				disabled={connecting}
				onclick={handleConnect}
			>
				<Plus class="h-3 w-3" />
				{connecting ? 'Redirecting to GitHub…' : 'Add Another App'}
			</Button>
		</div>
	</div>
{/if}
