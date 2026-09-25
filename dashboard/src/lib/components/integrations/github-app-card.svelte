<script lang="ts">
	import {
		listGitHubRepos,
		listGitHubBranches,
		updateGitHubApp,
		type GitHubApp,
		type GitHubRepo,
		type GitHubBranch
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import Github from '~icons/ri/github-fill';
	import Trash2 from '~icons/ri/delete-bin-line';
	import ChevronDown from '~icons/ri/arrow-down-s-line';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import GitBranch from '~icons/ri/git-branch-line';
	import ExternalLink from '~icons/ri/external-link-line';
	import Lock from '~icons/ri/lock-line';
	import Pencil from '~icons/ri/pencil-line';
	import Check from '~icons/ri/check-line';
	import X from '~icons/ri/close-line';
	import RefreshCw from '~icons/ri/refresh-line';

	interface Props {
		app: GitHubApp;
		disconnecting: boolean;
		onDisconnect: () => void;
		onInstall: () => void;
		onRename?: (updated: GitHubApp) => void;
	}

	let { app, disconnecting, onDisconnect, onInstall, onRename }: Props = $props();

	// ── Inline rename ─────────────────────────────────────────────────────────

	let editingName = $state(false);
	let nameInput = $state('');
	let nameSaving = $state(false);
	let nameError = $state('');
	let nameInputEl = $state<HTMLInputElement | null>(null);

	$effect(() => {
		if (editingName) nameInputEl?.focus();
	});
	let localName = $state<string | null>(null);
	let displayName = $derived(localName ?? app.name);

	function startEditName() {
		nameInput = displayName;
		nameError = '';
		editingName = true;
	}

	function cancelEditName() {
		editingName = false;
		nameError = '';
	}

	async function saveEditName() {
		const trimmed = nameInput.trim();
		if (!trimmed || trimmed === displayName) {
			cancelEditName();
			return;
		}
		nameSaving = true;
		nameError = '';
		try {
			const updated = await updateGitHubApp(app.id, { name: trimmed });
			localName = updated.name;
			onRename?.(updated);
			editingName = false;
		} catch (e: any) {
			nameError = e.message ?? 'Failed to rename';
		} finally {
			nameSaving = false;
		}
	}

	function handleNameKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') saveEditName();
		if (e.key === 'Escape') cancelEditName();
	}

	// ── Repos ─────────────────────────────────────────────────────────────────

	let reposExpanded = $state(false);
	let repos = $state<GitHubRepo[]>([]);
	let reposLoading = $state(false);
	let reposError = $state('');

	// ── Branches ──────────────────────────────────────────────────────────────

	let expandedRepoKey = $state<string | null>(null);
	let branchesByRepo = $state<Record<string, GitHubBranch[]>>({});
	let branchesLoading = $state<Record<string, boolean>>({});

	async function loadRepos() {
		reposLoading = true;
		reposError = '';
		try {
			repos = await listGitHubRepos(app.id);
		} catch (e: any) {
			reposError = e.message ?? 'Failed to load repositories';
		} finally {
			reposLoading = false;
		}
	}

	async function toggleRepos() {
		reposExpanded = !reposExpanded;
		if (reposExpanded && repos.length === 0 && !reposError) {
			await loadRepos();
		}
	}

	async function refreshRepos() {
		await loadRepos();
	}

	async function toggleBranches(repo: GitHubRepo) {
		const key = `${app.id}/${repo.full_name}`;
		if (expandedRepoKey === key) {
			expandedRepoKey = null;
			return;
		}
		expandedRepoKey = key;
		if (!branchesByRepo[key]) {
			branchesLoading[key] = true;
			const [owner, repoName] = repo.full_name.split('/');
			try {
				branchesByRepo[key] = await listGitHubBranches(app.id, owner, repoName);
			} catch {
				branchesByRepo[key] = [];
			} finally {
				branchesLoading[key] = false;
			}
		}
	}

	// ── Timestamps ────────────────────────────────────────────────────────────

	function formatDate(iso: string) {
		return new Date(iso).toLocaleDateString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric'
		});
	}
</script>

<div class="space-y-0 border">
	<!-- Header row -->
	<div class="flex items-center justify-between border-b bg-muted/20 px-4 py-3">
		<div class="group flex items-center gap-3">
			<Github class="h-5 w-5" />
			<div class="flex flex-col gap-0.5">
				{#if editingName}
					<div class="flex items-center gap-1">
						<input
							class="h-6 rounded border bg-background px-1.5 text-sm font-bold uppercase focus:ring-1 focus:ring-ring focus:outline-none"
							bind:this={nameInputEl}
							bind:value={nameInput}
							onkeydown={handleNameKeydown}
							disabled={nameSaving}
						/>
						<button
							class="text-success hover:opacity-80 disabled:opacity-40"
							onclick={saveEditName}
							disabled={nameSaving}
							aria-label="Save name"
						>
							<Check class="h-3.5 w-3.5" />
						</button>
						<button
							class="text-muted-foreground hover:opacity-80 disabled:opacity-40"
							onclick={cancelEditName}
							disabled={nameSaving}
							aria-label="Cancel"
						>
							<X class="h-3.5 w-3.5" />
						</button>
					</div>
					{#if nameError}
						<span class="text-[10px] text-destructive">{nameError}</span>
					{/if}
				{:else}
					<div class="flex items-center gap-1.5">
						<span class="text-sm font-bold uppercase">{displayName}</span>
						<button
							class="text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground focus:opacity-100"
							onclick={startEditName}
							aria-label="Rename app"
						>
							<Pencil class="h-3 w-3" />
						</button>
					</div>
				{/if}
				<span class="font-mono text-xs text-muted-foreground">/{app.slug}</span>
			</div>
		</div>
		<div class="flex items-center gap-2">
			{#if app.installed}
				<Badge variant="outline" class="border-success text-xs text-success">Installed</Badge>
			{:else}
				<Badge variant="outline" class="border-warn text-xs text-warn">Pending Install</Badge>
			{/if}
			<!-- View on GitHub -->
			<Button
				variant="ghost"
				size="sm"
				class="h-7 text-xs text-muted-foreground uppercase"
				href="https://github.com/settings/apps/{app.slug}"
				target="_blank"
				rel="noopener noreferrer"
				title="View app settings on GitHub"
			>
				<ExternalLink class="h-3 w-3" />
				GitHub
			</Button>
			{#if app.installed}
				<Button
					variant="outline"
					size="sm"
					class="h-7 text-xs uppercase"
					href={app.install_url}
					target="_blank"
					rel="noopener noreferrer"
				>
					<ExternalLink class="h-3 w-3" />
					Add Repos
				</Button>
			{/if}
			<Button
				variant="outline"
				size="sm"
				class="h-7 text-xs text-destructive uppercase hover:text-destructive"
				disabled={disconnecting}
				onclick={onDisconnect}
			>
				<Trash2 class="h-3 w-3" />
				{disconnecting ? 'Removing…' : 'Disconnect'}
			</Button>
		</div>
	</div>

	<!-- Detail cells -->
	<div class="grid grid-cols-2 gap-px bg-border text-xs sm:grid-cols-3 lg:grid-cols-5">
		<div class="flex flex-col gap-0.5 bg-background px-4 py-3">
			<span class="text-muted-foreground uppercase">App ID</span>
			<span class="font-mono">{app.app_id}</span>
		</div>
		<div class="flex flex-col gap-0.5 bg-background px-4 py-3">
			<span class="text-muted-foreground uppercase">Client ID</span>
			<span class="truncate font-mono">{app.client_id}</span>
		</div>
		<div class="flex flex-col gap-0.5 bg-background px-4 py-3">
			<span class="text-muted-foreground uppercase">Installation ID</span>
			<span class="font-mono">{app.installation_id ?? '—'}</span>
		</div>
		<div class="flex flex-col gap-0.5 bg-background px-4 py-3">
			<span class="text-muted-foreground uppercase">Created</span>
			<span class="font-mono">{formatDate(app.created_at)}</span>
		</div>
		<div class="flex flex-col gap-0.5 bg-background px-4 py-3">
			<span class="text-muted-foreground uppercase">Updated</span>
			<span class="font-mono">{formatDate(app.updated_at)}</span>
		</div>
	</div>

	<!-- Step 2: pending install CTA -->
	{#if !app.installed}
		<div class="border-t bg-muted/10 px-4 py-4">
			<p class="mb-1 text-xs font-medium">Step 2 of 2 — Install on repositories</p>
			<p class="mb-3 text-xs text-muted-foreground">
				Grant the app access to the repositories you want to deploy. You'll be taken to GitHub to
				choose which repos to install it on. Return here when done — this page will refresh
				automatically.
			</p>
			<Button size="sm" class="h-7 text-xs uppercase" onclick={onInstall}>
				<ExternalLink class="h-3 w-3" />
				Install on Repos
			</Button>
		</div>
	{/if}

	<!-- Repos accordion (only when installed) -->
	{#if app.installed}
		<div class="flex items-center border-t bg-muted/10">
			<button
				class="flex flex-1 items-center gap-2 px-4 py-2.5 text-xs font-medium tracking-wide uppercase hover:bg-muted/30"
				onclick={toggleRepos}
			>
				{#if reposExpanded}
					<ChevronDown class="h-3.5 w-3.5" />
				{:else}
					<ChevronRight class="h-3.5 w-3.5" />
				{/if}
				<span>Repositories</span>
				{#if repos.length > 0}
					<span class="rounded-full bg-muted px-1.5 py-0.5 text-[10px] font-medium tabular-nums">
						{repos.length}
					</span>
				{/if}
			</button>
			{#if reposExpanded}
				<button
					class="px-3 py-2.5 text-muted-foreground hover:text-foreground disabled:opacity-40"
					onclick={refreshRepos}
					disabled={reposLoading}
					aria-label="Refresh repositories"
					title="Refresh repositories"
				>
					<RefreshCw class="h-3.5 w-3.5 {reposLoading ? 'animate-spin' : ''}" />
				</button>
			{/if}
		</div>

		{#if reposExpanded}
			<div class="border-t">
				{#if reposLoading}
					<div class="py-6 text-center text-xs text-muted-foreground">Loading repos…</div>
				{:else if reposError}
					<div class="px-4 py-3 text-xs text-destructive">{reposError}</div>
				{:else if repos.length === 0}
					<div class="px-4 py-6 text-center text-xs text-muted-foreground">
						No repositories accessible. Check your installation permissions.
					</div>
				{:else}
					{#each repos as repo (repo.id)}
						{@const repoKey = `${app.id}/${repo.full_name}`}
						<div class="border-b last:border-b-0">
							<button
								class="flex w-full items-center gap-2 px-4 py-2.5 text-left text-xs hover:bg-muted/20"
								onclick={() => toggleBranches(repo)}
							>
								{#if expandedRepoKey === repoKey}
									<ChevronDown class="h-3 w-3 shrink-0 text-muted-foreground" />
								{:else}
									<ChevronRight class="h-3 w-3 shrink-0 text-muted-foreground" />
								{/if}
								<span class="min-w-0 flex-1 truncate font-mono font-medium">{repo.full_name}</span>
								{#if repo.private}
									<Lock class="h-3 w-3 shrink-0 text-muted-foreground" />
								{/if}
								<span class="shrink-0 text-muted-foreground">{repo.default_branch}</span>
							</button>

							{#if expandedRepoKey === repoKey}
								<div class="border-t bg-muted/10 px-4 py-3">
									{#if branchesLoading[repoKey]}
										<div class="py-2 text-center text-xs text-muted-foreground">
											Loading branches…
										</div>
									{:else}
										<div class="flex flex-wrap gap-1.5">
											{#each branchesByRepo[repoKey] ?? [] as branch (branch.name)}
												<div
													class="flex items-center gap-1 border px-1.5 py-0.5 font-mono text-[10px]"
												>
													<GitBranch class="h-2.5 w-2.5 text-muted-foreground" />
													{branch.name}
													{#if branch.name === repo.default_branch}
														<span class="text-muted-foreground">(default)</span>
													{/if}
												</div>
											{/each}
										</div>
										<p class="mt-2 text-[10px] text-muted-foreground">
											To deploy from this repo, link it in the app's Source settings.
										</p>
									{/if}
								</div>
							{/if}
						</div>
					{/each}
				{/if}
			</div>
		{/if}
	{/if}
</div>
