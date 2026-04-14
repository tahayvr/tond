<script lang="ts">
	import { page } from '$app/state';
	import Github from '@lucide/svelte/icons/github';
	import Package from '@lucide/svelte/icons/package';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import GitHubAppList from '$lib/components/integrations/github-app-list.svelte';
	import RegistryCredentialList from '$lib/components/integrations/registry-credential-list.svelte';
	import type { GitHubApp, RegistryCredential } from '$lib/api';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let sourceData = $derived(
		((data as { sourceData?: unknown }).sourceData ?? null) as {
			githubApps?: GitHubApp[];
			credentials?: RegistryCredential[];
		} | null
	);

	const source = $derived(page.params.source);

	const PAGE_META: Record<string, { title: string; icon: typeof Github }> = {
		github: { title: 'GitHub', icon: Github },
		registry: { title: 'Registry Credentials', icon: Package }
	};

	const meta = $derived(PAGE_META[source!] ?? { title: source ?? '', icon: Package });

	// ── GitHub App list ref (for silent refresh on focus) ─────────────────────

	let githubList = $state<GitHubAppList>();

	// Auto-refresh when the tab regains focus — picks up installation_id changes
	$effect(() => {
		if (source !== 'github') return;
		const handleFocus = () => githubList?.silentRefresh();
		window.addEventListener('focus', handleFocus);
		return () => window.removeEventListener('focus', handleFocus);
	});

	// Handle mock GitHub OAuth callback (dev only)
	$effect(() => {
		if (source !== 'github') return;
		const mockConnected = page.url.searchParams.get('mock_github_connected');
		if (mockConnected === '1') {
			// Reload the list so the newly-created (pending) app appears
			githubList?.load();
			const url = new URL(window.location.href);
			url.searchParams.delete('mock_github_connected');
			window.history.replaceState({}, '', url.toString());
		}
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-8">
	<!-- Header with back link -->
	<div class="flex h-14 items-center gap-4">
		<a href="/integrations" class="text-muted-foreground hover:text-foreground">
			<ChevronLeft class="h-5 w-5" />
		</a>
		<div class="flex items-center gap-3">
			<meta.icon class="h-6 w-6" />
			<h2 class="text-3xl font-extrabold uppercase">{meta.title}</h2>
		</div>
	</div>

	{#if source === 'github'}
		<GitHubAppList bind:this={githubList} initialApps={sourceData?.githubApps ?? []} />
	{:else if source === 'registry'}
		<RegistryCredentialList initialCredentials={sourceData?.credentials ?? []} />
	{:else}
		<p class="text-sm text-muted-foreground">Unknown integration: <code>{source}</code></p>
	{/if}
</div>
