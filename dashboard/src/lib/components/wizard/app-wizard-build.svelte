<script lang="ts">
	// Build step (git sources): branch, build method, base directory, static
	// output and auto-deploy. The branch list comes from the repository, with
	// its default branch preselected; typing any branch name still works.
	import { untrack } from 'svelte';
	import { listRemoteBranches } from '$lib/api';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';
	import BranchIcon from '~icons/ri/git-branch-line';

	interface Props {
		buildMethod: 'dockerfile' | 'tbuild';
		autoDeploy: boolean;
		/** Branch to deploy (auto-deploy watches the same branch). */
		branch: string;
		baseDir: string;
		staticBuild: boolean;
		sourceSubType: SourceSubType;
		sourceUrl: string;
		githubAppId: string | null;
	}

	let {
		buildMethod = $bindable<'dockerfile' | 'tbuild'>('tbuild'),
		autoDeploy = $bindable(false),
		branch = $bindable(''),
		baseDir = $bindable(''),
		staticBuild = $bindable(false),
		sourceSubType,
		sourceUrl,
		githubAppId
	}: Props = $props();

	let isGit = $derived(
		sourceSubType === 'git-public' ||
			sourceSubType === 'git-github-app' ||
			sourceSubType === 'template'
	);

	let branches = $state<string[]>([]);
	let defaultBranch = $state<string | null>(null);
	let branchesLoading = $state(false);
	let branchesNote = $state('');
	let loadedFor = '';

	async function loadBranches(url: string, appId: string | null) {
		branchesLoading = true;
		branchesNote = '';
		try {
			const result = await listRemoteBranches(url, appId ? { github_app_id: appId } : {});
			branches = result.branches;
			defaultBranch = result.default_branch;
			if (!branch.trim()) branch = result.default_branch ?? result.branches[0] ?? 'main';
		} catch (e: any) {
			branches = [];
			defaultBranch = null;
			branchesNote = e.message ?? "Couldn't list branches";
			if (!branch.trim()) branch = 'main';
		} finally {
			branchesLoading = false;
		}
	}

	$effect(() => {
		const url = sourceUrl.trim();
		const appId = githubAppId;
		if (!isGit || !/^https?:\/\//.test(url)) return;
		const key = `${url}|${appId ?? ''}`;
		if (key === loadedFor) return;
		loadedFor = key;
		untrack(() => loadBranches(url, appId));
	});

	const BUILD_METHODS: { value: 'tbuild' | 'dockerfile'; label: string; desc: string }[] = [
		{
			value: 'tbuild',
			label: 'TBuild',
			desc: 'Generate a Dockerfile from the repo and build it'
		},
		{
			value: 'dockerfile',
			label: 'Dockerfile',
			desc: 'Use the Dockerfile committed at the repository root'
		}
	];

	let selectedDesc = $derived(BUILD_METHODS.find((m) => m.value === buildMethod)?.desc ?? '');
</script>

<div class="space-y-6">
	{#if isGit}
		<div class="space-y-2">
			<Label for="branch">Branch *</Label>
			<div class="flex max-w-md items-center gap-2">
				<BranchIcon class="size-4 shrink-0 text-muted-foreground" />
				<Input
					id="branch"
					type="text"
					bind:value={branch}
					list="branch-options"
					placeholder={branchesLoading ? 'Loading branches…' : 'main'}
					class="font-mono"
					autocomplete="off"
					spellcheck={false}
				/>
				<datalist id="branch-options">
					{#each branches as b (b)}
						<option value={b}>{b === defaultBranch ? 'default branch' : ''}</option>
					{/each}
				</datalist>
			</div>
			<p class="text-xs text-muted-foreground">
				{#if branchesLoading}
					Reading the repository's branches…
				{:else if branchesNote}
					{branchesNote}
				{:else if defaultBranch}
					{branches.length} branch{branches.length === 1 ? '' : 'es'}; the repository's default is
					<code>{defaultBranch}</code>.
				{:else}
					The branch Tond clones and deploys.
				{/if}
			</p>
		</div>

		<Separator class="bg-border" />
	{/if}

	<div class="flex flex-wrap items-center gap-2">
		<Label for="build-method">Build Method:</Label>
		<Select.Root type="single" bind:value={buildMethod}>
			<Select.Trigger id="build-method">
				{BUILD_METHODS.find((m) => m.value === buildMethod)?.label ?? 'Select…'}
			</Select.Trigger>
			<Select.Content>
				{#each BUILD_METHODS as method (method.value)}
					<Select.Item value={method.value}>{method.label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
		{#if selectedDesc}
			<p class="text-xs text-muted-foreground">{selectedDesc}</p>
		{/if}
	</div>

	<Separator class="bg-border" />

	<div class="space-y-2">
		<Label for="base-dir">Base Directory</Label>
		<Input id="base-dir" type="text" bind:value={baseDir} placeholder="/" class="font-mono" />
		<p class="text-xs text-muted-foreground">
			Directory to use as root. Useful for monorepos. Leave empty to use the repository root.
		</p>
	</div>

	<Separator class="bg-border" />

	<div class="flex items-center justify-between gap-4 sm:w-1/2">
		<div class="space-y-0.5">
			<Label>Static Build?</Label>
			<p class="text-xs text-muted-foreground">
				If your application is a static site or the final build assets should be served as a static
				site, enable this.
			</p>
		</div>
		<Checkbox bind:checked={staticBuild} />
	</div>

	{#if isGit}
		<Separator class="bg-border" />

		<div class="flex items-center justify-between gap-4">
			<div class="space-y-0.5">
				<Label>Auto Deploy</Label>
				<p class="text-xs text-muted-foreground">
					Deploy automatically when you push to <code>{branch.trim() || 'the branch above'}</code> (needs
					a webhook or a GitHub App).
				</p>
			</div>
			<Checkbox bind:checked={autoDeploy} />
		</div>
	{/if}
</div>
