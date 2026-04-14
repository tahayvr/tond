<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import Container from '@lucide/svelte/icons/container';
	import LayoutTemplate from '@lucide/svelte/icons/layout-template';
	import Github from '@lucide/svelte/icons/github';
	import Lock from '@lucide/svelte/icons/lock';
	import Globe from '@lucide/svelte/icons/globe';
	import FileCode from '@lucide/svelte/icons/file-code';
	import Layers from '@lucide/svelte/icons/layers';
	import Box from '@lucide/svelte/icons/box';
	import { Button } from '$lib/components/ui/button/index.js';
	import {
		listGitHubApps,
		listGitHubRepos,
		listGitHubBranches,
		type GitHubApp,
		type GitHubRepo,
		type TemplateCatalogEntry
	} from '$lib/api';

	export type SourceSubType =
		| 'git-public'
		| 'git-github-app'
		| 'dockerfile'
		| 'compose'
		| 'image'
		| 'template';

	interface Props {
		name: string;
		nameManuallyEdited: boolean;
		sourceSubType: SourceSubType;
		sourceUrl: string;
		githubAppId: string | null;
		dockerfileContent: string;
		composeContent: string;
		selectedTemplateId: string;
		templates: TemplateCatalogEntry[];
		envVars: Record<string, string>;
		onTemplateSelect: (templateId: string) => void;
	}

	let {
		name = $bindable(''),
		nameManuallyEdited,
		sourceSubType = $bindable('git-public'),
		sourceUrl = $bindable(''),
		githubAppId = $bindable(null),
		dockerfileContent = $bindable(''),
		composeContent = $bindable(''),
		selectedTemplateId = $bindable(''),
		templates,
		envVars = $bindable({}),
		onTemplateSelect
	}: Props = $props();

	// Derive the top-level category from the sub-type
	let sourceCategory = $derived<'git' | 'docker' | 'template'>(
		sourceSubType === 'git-public' || sourceSubType === 'git-github-app'
			? 'git'
			: sourceSubType === 'template'
				? 'template'
				: 'docker'
	);
	let selectedTemplate = $derived(
		selectedTemplateId
			? (templates.find((template) => template.id === selectedTemplateId) ?? null)
			: null
	);
	let templateRequiredInputs = $derived(
		selectedTemplate
			? selectedTemplate.required_env.filter(
					(env: TemplateCatalogEntry['required_env'][number]) => !env.value
				)
			: []
	);

	// GitHub App picker state
	let githubApps = $state<GitHubApp[]>([]);
	let githubAppsLoaded = $state(false);
	let githubPickerRepos = $state<Record<string, GitHubRepo[]>>({});
	let githubPickerBranches = $state<Record<string, string[]>>({});
	let selectedGithubAppId = $state<string | null>(null);
	let selectedGithubRepo = $state<GitHubRepo | null>(null);
	let selectedGithubBranch = $state<string | null>(null);
	let githubPickerReposLoading = $state(false);
	let githubPickerBranchesLoading = $state(false);

	function normalizeName(value: string): string {
		return value
			.toLowerCase()
			.replace(/\s+/g, '-')
			.replace(/[^a-z0-9-]/g, '')
			.replace(/-{2,}/g, '-')
			.replace(/^-+|-+$/g, '');
	}

	function generateNameFromSource(repo: GitHubRepo, branch: string): string {
		const repoName = repo.full_name.split('/')[1] ?? repo.full_name;
		const isDefault = branch === repo.default_branch;
		const raw = isDefault ? repoName : `${repoName}-${branch}`;
		return normalizeName(raw);
	}

	let installedApps = $derived(githubApps.filter((a) => a.installed));

	$effect(() => {
		if (sourceSubType === 'git-github-app' && !githubAppsLoaded) {
			loadGitHubApps();
		}
	});

	async function loadGitHubApps() {
		try {
			githubApps = await listGitHubApps();
			githubAppsLoaded = true;
		} catch {
			githubAppsLoaded = true;
		}
	}

	async function handleGithubAppChange(appId: string | null) {
		selectedGithubAppId = appId;
		selectedGithubRepo = null;
		selectedGithubBranch = null;
		githubAppId = null;
		sourceUrl = '';
		if (!appId) return;
		if (!githubPickerRepos[appId]) {
			githubPickerReposLoading = true;
			try {
				githubPickerRepos[appId] = await listGitHubRepos(appId);
			} catch {
				githubPickerRepos[appId] = [];
			} finally {
				githubPickerReposLoading = false;
			}
		}
	}

	async function handleGithubRepoChange(repo: GitHubRepo | null) {
		selectedGithubRepo = repo;
		selectedGithubBranch = null;
		if (!selectedGithubAppId || !repo) {
			githubAppId = null;
			sourceUrl = '';
			return;
		}
		const [owner, repoName] = repo.full_name.split('/');
		const key = `${selectedGithubAppId}/${repo.full_name}`;
		if (!githubPickerBranches[key]) {
			githubPickerBranchesLoading = true;
			try {
				const branches = await listGitHubBranches(selectedGithubAppId, owner, repoName);
				githubPickerBranches[key] = branches.map((b) => b.name);
			} catch {
				githubPickerBranches[key] = [];
			} finally {
				githubPickerBranchesLoading = false;
			}
		}
		selectedGithubBranch = repo.default_branch;
		githubAppId = selectedGithubAppId;
		sourceUrl = repo.clone_url;
		if (!nameManuallyEdited) {
			name = generateNameFromSource(repo, repo.default_branch);
		}
	}

	function handleGithubBranchChange(branch: string | null) {
		selectedGithubBranch = branch;
		if (!nameManuallyEdited && selectedGithubRepo && branch) {
			name = generateNameFromSource(selectedGithubRepo, branch);
		}
	}

	function handleClearGithubPicker() {
		selectedGithubAppId = null;
		selectedGithubRepo = null;
		selectedGithubBranch = null;
		githubAppId = null;
		sourceUrl = '';
		if (!nameManuallyEdited) {
			name = '';
		}
	}

	function selectSubType(sub: SourceSubType) {
		sourceSubType = sub;
		// Reset fields that belong to the previous sub-type
		if (sub !== 'git-github-app') {
			handleClearGithubPicker();
		}
		if (sub !== 'template') {
			selectedTemplateId = '';
		}
		if (sub !== 'git-public' && sub !== 'git-github-app' && sub !== 'image') {
			sourceUrl = '';
		}
	}

	function selectTemplate(templateId: string) {
		sourceSubType = 'template';
		selectedTemplateId = templateId;
		onTemplateSelect(templateId);
	}

	const GIT_SUB_OPTIONS: {
		id: SourceSubType;
		label: string;
		description: string;
		icon: typeof Globe;
	}[] = [
		{
			id: 'git-public',
			label: 'Public Repo',
			description:
				'You can deploy any kind of public repositories from the supported git providers.',
			icon: Globe
		},
		{
			id: 'git-github-app',
			label: 'Private Repo (GitHub App)',
			description: 'You can deploy public & private repositories through your GitHub Apps.',
			icon: Github
		}
	];

	const DOCKER_SUB_OPTIONS: {
		id: SourceSubType;
		label: string;
		description: string;
		icon: typeof Globe;
	}[] = [
		{
			id: 'dockerfile',
			label: 'Dockerfile',
			description: 'You can deploy a simple Dockerfile, without git.',
			icon: FileCode
		},
		{
			id: 'compose',
			label: 'Docker Compose',
			description: 'You can deploy complex applications easily with Docker Compose, without Git.',
			icon: Layers
		},
		{
			id: 'image',
			label: 'Docker Image',
			description: 'You can deploy an existing Docker Image from any Registry, without Git.',
			icon: Box
		}
	];
</script>

<div class="space-y-6">
	<!-- Category selection: Git vs Docker -->
	<div class="space-y-4">
		<Label>Source</Label>
		<div class="grid gap-3 sm:grid-cols-3">
			<Button
				variant={sourceCategory === 'git' ? 'default' : 'outline'}
				onclick={() => selectSubType('git-public')}
			>
				<GitBranch />
				Git
			</Button>

			<Button
				variant={sourceCategory === 'docker' ? 'default' : 'outline'}
				onclick={() => selectSubType('dockerfile')}
			>
				<Container />
				Docker
			</Button>

			<Button
				variant={sourceCategory === 'template' ? 'default' : 'outline'}
				onclick={() => selectSubType('template')}
			>
				<LayoutTemplate />
				Templates
			</Button>
		</div>
	</div>

	<!-- Sub-option cards -->
	{#if sourceCategory === 'git'}
		<div class="grid gap-3 sm:grid-cols-2">
			{#each GIT_SUB_OPTIONS as opt (opt.id)}
				<button
					class="flex flex-col items-start gap-2 rounded-lg border-2 p-4 text-left transition-all {sourceSubType ===
					opt.id
						? 'border-primary bg-muted'
						: 'border-border hover:border-input'}"
					onclick={() => selectSubType(opt.id)}
				>
					<opt.icon class="h-5 w-5 text-muted-foreground" />
					<div>
						<div class="text-sm font-semibold">{opt.label}</div>
						<p class="text-xs text-muted-foreground">{opt.description}</p>
					</div>
				</button>
			{/each}
		</div>
	{:else if sourceCategory === 'docker'}
		<div class="grid gap-3 sm:grid-cols-3">
			{#each DOCKER_SUB_OPTIONS as opt (opt.id)}
				<button
					class="flex flex-col items-start gap-2 rounded-lg border-2 p-4 text-left transition-all {sourceSubType ===
					opt.id
						? 'border-primary bg-muted'
						: 'border-border hover:border-input'}"
					onclick={() => selectSubType(opt.id)}
				>
					<opt.icon class="h-5 w-5 text-muted-foreground" />
					<div>
						<div class="text-sm font-semibold">{opt.label}</div>
						<p class="text-xs text-muted-foreground">{opt.description}</p>
					</div>
				</button>
			{/each}
		</div>
	{:else}
		<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
			{#each templates as template (template.id)}
				<button
					class="flex flex-col items-start gap-2 rounded-lg border-2 p-4 text-left transition-all {selectedTemplateId ===
					template.id
						? 'border-primary bg-muted'
						: 'border-border hover:border-input'}"
					onclick={() => selectTemplate(template.id)}
				>
					<LayoutTemplate class="h-5 w-5 text-muted-foreground" />
					<div>
						<div class="text-sm font-semibold">{template.name}</div>
						<p class="mt-1 text-xs text-muted-foreground">{template.description}</p>
					</div>
					<div class="flex flex-wrap gap-1 pt-1">
						<span class="rounded border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
							{template.source.kind}
						</span>
						<span class="rounded border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
							{template.deploy.method}
						</span>
						<span class="rounded border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
							:{template.deploy.default_port}
						</span>
					</div>
				</button>
			{/each}
		</div>
	{/if}

	<!-- Context-sensitive content area -->

	{#if sourceSubType === 'template' && selectedTemplate}
		<div class="space-y-4 rounded-md border bg-muted/10 px-4 py-4">
			<div class="space-y-1">
				<p class="text-xs font-medium text-muted-foreground uppercase">Selected Template</p>
				<p class="text-sm font-medium">{selectedTemplate.name}</p>
				{#if selectedTemplate.setup_notes}
					<p class="text-xs text-muted-foreground">{selectedTemplate.setup_notes}</p>
				{/if}
			</div>

			{#if selectedTemplate.dependencies.length > 0}
				<div class="space-y-2">
					<Label class="text-xs">Dependencies</Label>
					<div class="flex flex-wrap gap-2">
						{#each selectedTemplate.dependencies as dependency, index (`${dependency.kind}-${index}`)}
							<span class="rounded border px-2 py-1 font-mono text-xs">
								{dependency.kind}:{dependency.name ?? dependency.engine ?? 'unnamed'}
							</span>
						{/each}
					</div>
				</div>
			{/if}

			{#if templateRequiredInputs.length > 0}
				<div class="space-y-3">
					<Label class="text-xs">Required Template Inputs</Label>
					<div class="grid gap-3 sm:grid-cols-2">
						{#each templateRequiredInputs as env (env.key)}
							<div class="space-y-2">
								<Label for={`template-env-${env.key}`}>{env.key}</Label>
								<Input
									id={`template-env-${env.key}`}
									value={envVars[env.key] ?? ''}
									oninput={(event) => {
										envVars = {
											...envVars,
											[env.key]: (event.target as HTMLInputElement).value
										};
									}}
									placeholder={env.description}
									class="font-mono"
								/>
								<p class="text-xs text-muted-foreground">{env.description}</p>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	{/if}

	{#if sourceSubType === 'git-github-app'}
		<!-- GitHub App picker -->
		<div class="space-y-3 rounded-md border bg-muted/10 px-4 py-3">
			<p class="text-xs font-medium text-muted-foreground uppercase">Link a GitHub App</p>
			<p class="text-xs text-muted-foreground">
				Select a connected GitHub App and repository to enable private repo access and auto-fill the
				Git URL.
			</p>

			<div class="flex flex-wrap items-end gap-3">
				<!-- App selector -->
				<div class="grid gap-1.5">
					<Label class="text-xs">GitHub App</Label>
					<Select.Root
						type="single"
						value={selectedGithubAppId ?? ''}
						onValueChange={(v) => handleGithubAppChange(v || null)}
					>
						<Select.Trigger class="h-8 w-44 text-xs">
							{selectedGithubAppId
								? (installedApps.find((a) => a.id === selectedGithubAppId)?.name ??
									selectedGithubAppId)
								: 'Select app…'}
						</Select.Trigger>
						<Select.Content>
							{#each installedApps as ghApp (ghApp.id)}
								<Select.Item value={ghApp.id} class="text-xs">{ghApp.name}</Select.Item>
							{/each}
						</Select.Content>
					</Select.Root>
				</div>

				<!-- Repo selector -->
				{#if selectedGithubAppId}
					<div class="grid gap-1.5">
						<Label class="text-xs">Repository</Label>
						{#if githubPickerReposLoading}
							<p class="flex h-8 items-center text-xs text-muted-foreground">Loading…</p>
						{:else}
							<Select.Root
								type="single"
								value={selectedGithubRepo?.full_name ?? ''}
								onValueChange={(v) => {
									const repo =
										githubPickerRepos[selectedGithubAppId ?? '']?.find((r) => r.full_name === v) ??
										null;
									handleGithubRepoChange(repo);
								}}
							>
								<Select.Trigger class="h-8 w-52 font-mono text-xs">
									{selectedGithubRepo?.full_name ?? 'Select repo…'}
								</Select.Trigger>
								<Select.Content>
									{#each githubPickerRepos[selectedGithubAppId] ?? [] as repo (repo.id)}
										<Select.Item value={repo.full_name} class="font-mono text-xs">
											{#if repo.private}
												<Lock class="mr-1 inline h-3 w-3 text-muted-foreground" />
											{/if}
											{repo.full_name}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						{/if}
					</div>
				{/if}

				<!-- Branch selector -->
				{#if selectedGithubRepo}
					{@const branchKey = `${selectedGithubAppId}/${selectedGithubRepo.full_name}`}
					<div class="grid gap-1.5">
						<Label class="text-xs">Branch</Label>
						{#if githubPickerBranchesLoading}
							<p class="flex h-8 items-center text-xs text-muted-foreground">Loading…</p>
						{:else}
							<Select.Root
								type="single"
								value={selectedGithubBranch ?? ''}
								onValueChange={(v) => handleGithubBranchChange(v || null)}
							>
								<Select.Trigger class="h-8 w-36 font-mono text-xs">
									{#if selectedGithubBranch}
										<GitBranch class="mr-1 inline h-3 w-3" />
										{selectedGithubBranch}
									{:else}
										Select branch…
									{/if}
								</Select.Trigger>
								<Select.Content>
									{#each githubPickerBranches[branchKey] ?? [] as branch (branch)}
										<Select.Item value={branch} class="font-mono text-xs">
											{branch}
											{#if branch === selectedGithubRepo.default_branch}
												<span class="ml-1 text-muted-foreground">(default)</span>
											{/if}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						{/if}
					</div>
				{/if}
			</div>

			{#if selectedGithubRepo}
				<p class="font-mono text-[10px] text-muted-foreground">
					<Github class="mr-1 inline h-3 w-3" />{selectedGithubRepo.clone_url}
				</p>
				<button
					class="text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
					onclick={handleClearGithubPicker}
				>
					Clear selection
				</button>
			{/if}
		</div>
	{/if}

	<!-- Git URL (public repo only) -->
	{#if sourceSubType === 'git-public'}
		<div class="space-y-2">
			<Label for="source-url">Git Repository URL *</Label>
			<Input
				id="source-url"
				type="text"
				bind:value={sourceUrl}
				placeholder="https://github.com/username/repo.git"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">
				HTTPS or SSH format (e.g. https://github.com/user/repo.git or git@github.com:user/repo.git)
			</p>
		</div>
	{/if}

	<!-- Dockerfile inline editor -->
	{#if sourceSubType === 'dockerfile'}
		<div class="space-y-2">
			<Label for="dockerfile-content">Dockerfile *</Label>
			<Textarea
				id="dockerfile-content"
				bind:value={dockerfileContent}
				placeholder="FROM node:20-alpine

WORKDIR /app

COPY package*.json ./
RUN npm install

COPY . .
RUN npm run build

EXPOSE 3000
CMD [&quot;node&quot;, &quot;dist/index.js&quot;]"
				class="min-h-64 font-mono text-xs"
				spellcheck={false}
			/>
			<p class="text-xs text-muted-foreground">
				Paste your Dockerfile content. It will be built directly on the server without a git
				repository.
			</p>
		</div>
	{/if}

	<!-- Docker Compose inline editor -->
	{#if sourceSubType === 'compose'}
		<div class="space-y-2">
			<Label for="compose-content">docker-compose.yml *</Label>
			<Textarea
				id="compose-content"
				bind:value={composeContent}
				placeholder="services:
  web:
    image: nginx:alpine
    ports:
      - &quot;80:80&quot;
    volumes:
      - ./html:/usr/share/nginx/html"
				class="min-h-64 font-mono text-xs"
				spellcheck={false}
			/>
			<p class="text-xs text-muted-foreground">
				Paste your docker-compose.yml content. The application will be started with
				<code class="rounded bg-muted px-1 font-mono">docker compose up --build -d</code>.
			</p>
		</div>
	{/if}

	<!-- Docker Image reference -->
	{#if sourceSubType === 'image'}
		<div class="space-y-2">
			<Label for="source-url">Docker Image *</Label>
			<Input
				id="source-url"
				type="text"
				bind:value={sourceUrl}
				placeholder="nginx:latest or registry.example.com/image:tag"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">
				Include tag (e.g. <code class="rounded bg-muted px-1 font-mono">:latest</code>). For private
				registries, select credentials in the Environment step.
			</p>
		</div>
	{/if}
</div>
