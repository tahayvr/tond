<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';

	interface Props {
		buildMethod: 'dockerfile' | 'tbuild';
		containerPort: number;
		autoDeploy: boolean;
		watchBranch: string;
		baseDir: string;
		staticBuild: boolean;
		sourceSubType: SourceSubType;
	}

	let {
		buildMethod = $bindable<'dockerfile' | 'tbuild'>('tbuild'),
		containerPort = $bindable(8080),
		autoDeploy = $bindable(false),
		watchBranch = $bindable('main'),
		baseDir = $bindable(''),
		staticBuild = $bindable(false),
		sourceSubType
	}: Props = $props();

	// Build method and auto-deploy are only relevant for git-based source types
	let isGit = $derived(sourceSubType === 'git-public' || sourceSubType === 'git-github-app');

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
	<div class="flex items-center gap-2">
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
		<Label for="container-port">Container Port *</Label>
		<Input id="container-port" type="number" bind:value={containerPort} min={1} max={65535} />
		<p class="text-xs text-muted-foreground">
			The port your application listens on inside the container. Default: 8080
		</p>
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

	<div class="flex w-1/2 items-center justify-between">
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

		<div class="space-y-4">
			<div class="flex items-center justify-between">
				<div class="space-y-0.5">
					<Label>Auto Deploy</Label>
					<p class="text-xs text-muted-foreground">
						Automatically deploy when changes are pushed to the watched branch
					</p>
				</div>
				<Checkbox bind:checked={autoDeploy} />
			</div>

			{#if autoDeploy}
				<div class="space-y-2">
					<Label for="watch-branch">Watch Branch</Label>
					<Input id="watch-branch" type="text" bind:value={watchBranch} placeholder="main" />
					<p class="text-xs text-muted-foreground">Branch to watch for automatic deployments</p>
				</div>
			{/if}
		</div>
	{/if}
</div>
