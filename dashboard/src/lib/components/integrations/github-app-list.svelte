<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import {
		listGitHubApps,
		getGitHubAppManifestUrl,
		deleteGitHubApp,
		getGitHubAppInstallUrl,
		type GitHubApp
	} from '$lib/api';
	import { safeUrl } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import GitHubAppCard from './github-app-card.svelte';
	import Plus from '~icons/ri/add-line';

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
		const target = apps.find((a) => a.id === id);
		error = '';
		await confirmAction({
			title: `Disconnect ${target?.name ?? 'this GitHub App'}?`,
			description:
				'Apps linked to it lose access to private repositories, push-to-deploy and PR previews. The app stays registered on GitHub; delete it there too if you no longer need it.',
			confirmLabel: 'Disconnect',
			pendingLabel: 'Disconnecting…',
			onconfirm: async () => {
				disconnectingId = id;
				try {
					await deleteGitHubApp(id);
					apps = apps.filter((a) => a.id !== id);
				} finally {
					disconnectingId = null;
				}
			}
		});
	}

	async function handleInstall(app: GitHubApp) {
		error = '';
		try {
			// The URL carries a single-use state bound to this admin session;
			// GitHub echoes it back so the installation is stored on this App.
			const url = safeUrl(await getGitHubAppInstallUrl(app.id));
			if (!url) throw new Error('Unexpected installation URL');
			window.location.href = url;
		} catch (e: any) {
			error = e.message ?? 'Failed to start GitHub App installation';
		}
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

<InlineError title="GitHub Error" message={error} />

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
