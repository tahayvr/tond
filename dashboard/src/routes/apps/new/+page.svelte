<script lang="ts">
	import PageTitle from '$lib/components/nav/page-title.svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		getSettings,
		createApp,
		deployApp,
		getSystemMetrics,
		type DeployStarted,
		type Readiness,
		type Project,
		type RegistryCredential,
		type VolumeInput
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { isAdmin } from '$lib/auth.svelte';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import ChevronRight from '~icons/ri/arrow-right-s-line';
	import ChevronLeft from '~icons/ri/arrow-left-s-line';
	import Rocket from '~icons/ri/rocket-line';
	import GitBranch from '~icons/ri/git-branch-line';
	import Network from '~icons/ri/node-tree';
	import Settings from '~icons/ri/settings-3-line';
	import CheckCircle from '~icons/ri/checkbox-circle-line';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import AppWizardSteps from '$lib/components/wizard/app-wizard-steps.svelte';
	import AppWizardSource from '$lib/components/wizard/app-wizard-source.svelte';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';
	import AppWizardBuild from '$lib/components/wizard/app-wizard-build.svelte';
	import AppWizardNetwork from '$lib/components/wizard/app-wizard-network.svelte';
	import AppWizardResources from '$lib/components/wizard/app-wizard-resources.svelte';
	import AppWizardEnv from '$lib/components/wizard/app-wizard-env.svelte';
	import AppWizardReview from '$lib/components/wizard/app-wizard-review.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	// Get project_id from query params if coming from project page
	const projectIdFromQuery = $derived(page.url.searchParams.get('project_id'));

	// Steps configuration
	const steps = [
		{ id: 'source', label: 'Source', icon: GitBranch },
		{ id: 'build', label: 'Build', icon: Settings },
		{ id: 'network', label: 'Network', icon: Network },
		{ id: 'resources', label: 'Resources', icon: Settings },
		{ id: 'env', label: 'Environment', icon: Settings },
		{ id: 'review', label: 'Review', icon: CheckCircle }
	];

	// Form state
	let currentStep = $state(0);
	let loading = $state(false);
	let error = $state('');
	let projects = $derived(data.projects as Project[]);
	let registryCredentials = $derived(data.registryCredentials as RegistryCredential[]);

	// Step 1: Source
	let name = $state('');
	let nameManuallyEdited = $state(false);
	let sourceSubType = $state<SourceSubType>('git-public');
	let sourceUrl = $state('');
	let selectedProjectId = $state<string>('');
	let githubAppId = $state<string | null>(null);
	let dockerfileContent = $state('');
	let composeContent = $state('');

	// Step 2: Build
	let buildMethod = $state<'dockerfile' | 'tbuild'>('tbuild');
	let containerPort = $state(8080);
	/** The user typed a port; detected ports no longer overwrite it. */
	let portTouched = $state(false);
	/** Web app (http), TCP service (tcp) or background worker (none). */
	let readiness = $state<Readiness>('http');
	/** Background workers: seconds the new container must stay up. */
	let readyAfterSecs = $state(10);
	let isWorker = $derived(readiness === 'none');
	let autoDeploy = $state(false);
	/** Branch to deploy; auto-deploy watches it too. Empty until the repo's default is known. */
	let branch = $state('');
	let baseDir = $state('');
	let staticBuild = $state(false);

	// Step 3: Network
	let domain = $state('');
	// From Settings: where apps without a custom domain are served, and the
	// dashboard domain (a custom domain on its site gets a warning).
	let appsDomain = $state<string | null>(null);
	let dashboardDomain = $state('');

	$effect(() => {
		(async () => {
			try {
				const settings = await getSettings();
				appsDomain = settings.apps_domain ?? '';
				dashboardDomain = settings.dashboard_domain ?? '';
			} catch {
				// Unknown: the network step then shows neutral copy.
			}
		})();
	});
	// No path by default: the deploy then waits for any answer on the port.
	let healthCheckPath = $state('');
	let healthCheckInterval = $state(30);
	let healthFailureThreshold = $state(3);
	let drainTimeout = $state(30);

	// Step 4: Resources
	let memoryLimit = $state<number | null>(null);
	let memoryUnit = $state<'mb' | 'gb'>('mb');
	let cpuLimit = $state<number | null>(null);
	// Host paths are assigned by the server (<data>/volumes/<app>/<name>).
	let volumes = $state<VolumeInput[]>([]);
	let newVolumePath = $state('');

	// Step 5: Environment
	let envVars = $state<Record<string, string>>({});
	let registryCredentialId = $state<string>('');

	/** Git-based source: branch and auto-deploy apply. */
	let isGitSource = $derived(sourceSubType === 'git-public' || sourceSubType === 'git-github-app');
	/** What the Deploy button is doing right now. */
	let createStage = $state('');
	/** Total server memory, to explain the default memory cap. */
	let hostMemoryBytes = $state<number | null>(null);

	// Initialize
	$effect(() => {
		setUiContext(projectsContext());
		if (projectIdFromQuery) {
			selectedProjectId = projectIdFromQuery;
		}
		return () => clearUiContext();
	});

	// The resources step explains the default memory cap for this server.
	$effect(() => {
		if (currentStep !== 3 || hostMemoryBytes !== null) return;
		getSystemMetrics()
			.then((m) => (hostMemoryBytes = m.mem_total || null))
			.catch(() => {});
	});

	// Build step (index 1) is irrelevant for docker source types
	let isBuildStepSkipped = $derived(
		sourceSubType === 'dockerfile' || sourceSubType === 'compose' || sourceSubType === 'image'
	);

	// Visible steps for the indicator — hide Build when skipped
	let visibleSteps = $derived(isBuildStepSkipped ? steps.filter((s) => s.id !== 'build') : steps);

	// Map a visible-step index back to the real step index
	function toRealStep(visibleIndex: number): number {
		return steps.indexOf(visibleSteps[visibleIndex]);
	}

	// Map the real step index to a visible-step index
	let visibleStepIndex = $derived(visibleSteps.indexOf(steps[currentStep]));

	function nextStep() {
		error = '';
		if (!name.trim()) {
			error = 'App name is required';
			return;
		}
		if (currentStep === 0 && !validateSourceStep()) return;
		if (currentStep === 2 && !validateNetworkStep()) return;
		let next = currentStep + 1;
		// Skip the build step when it's not applicable
		if (next === 1 && isBuildStepSkipped) next = 2;
		if (next < steps.length) currentStep = next;
	}

	function prevStep() {
		error = '';
		let prev = currentStep - 1;
		// Skip back over the build step when it's not applicable
		if (prev === 1 && isBuildStepSkipped) prev = 0;
		if (prev >= 0) currentStep = prev;
	}

	function handleNameInput(e: Event) {
		const raw = (e.target as HTMLInputElement).value;
		name = raw
			.toLowerCase()
			.replace(/\s+/g, '-')
			.replace(/[^a-z0-9-]/g, '')
			.replace(/-{2,}/g, '-');
		nameManuallyEdited = name.length > 0;
	}

	function validateSourceStep(): boolean {
		if (sourceSubType === 'git-public') {
			if (!sourceUrl.trim()) {
				error = 'Git URL is required';
				return false;
			}
			if (!isValidGitUrl(sourceUrl)) {
				error = 'Please enter a valid Git URL (https:// or git@ format)';
				return false;
			}
		} else if (sourceSubType === 'git-github-app') {
			if (!githubAppId) {
				error = 'Please select a repository from your GitHub App';
				return false;
			}
		} else if (sourceSubType === 'image') {
			if (!sourceUrl.trim()) {
				error = 'Docker image is required';
				return false;
			}
		} else if (sourceSubType === 'dockerfile') {
			if (!dockerfileContent.trim()) {
				error = 'Dockerfile content is required';
				return false;
			}
		} else if (sourceSubType === 'compose') {
			if (!composeContent.trim()) {
				error = 'docker-compose.yml content is required';
				return false;
			}
		}
		return true;
	}

	function validateNetworkStep(): boolean {
		if (isWorker) {
			if (!Number.isInteger(readyAfterSecs) || readyAfterSecs < 1 || readyAfterSecs > 600) {
				error = 'Enter how long the worker must stay up (1-600 seconds)';
				return false;
			}
			return true;
		}
		if (!Number.isInteger(containerPort) || containerPort < 1 || containerPort > 65535) {
			error = 'Enter the port your app listens on (1-65535)';
			return false;
		}
		if (domain && !isValidDomain(domain)) {
			error = 'Please enter a valid domain';
			return false;
		}
		if (healthCheckInterval < 5 || healthCheckInterval > 300) {
			error = 'Health check interval must be between 5 and 300 seconds';
			return false;
		}
		if (healthFailureThreshold < 1 || healthFailureThreshold > 10) {
			error = 'Health failure threshold must be between 1 and 10';
			return false;
		}
		return true;
	}

	function isValidGitUrl(url: string): boolean {
		return url.startsWith('https://') || url.startsWith('git@') || url.startsWith('http://');
	}

	function isValidDomain(domain: string): boolean {
		const pattern =
			/^[a-zA-Z0-9][a-zA-Z0-9-]{0,61}[a-zA-Z0-9](?:\.[a-zA-Z0-9][a-zA-Z0-9-]{0,61}[a-zA-Z0-9])*$/;
		return pattern.test(domain);
	}

	function addVolume() {
		const path = newVolumePath.trim();
		if (!path) return;
		volumes = [...volumes, { container_path: path }];
		newVolumePath = '';
	}

	function removeVolume(index: number) {
		volumes = volumes.filter((_, i) => i !== index);
	}

	async function handleCreate() {
		error = '';
		loading = true;
		createStage = 'Creating the app…';

		try {
			// Calculate memory limit in bytes
			let memoryLimitBytes: number | undefined;
			if (memoryLimit !== null) {
				memoryLimitBytes =
					memoryUnit === 'gb' ? memoryLimit * 1024 * 1024 * 1024 : memoryLimit * 1024 * 1024;
			}

			// Calculate CPU limit in nano_cpus (1 CPU = 1e9 nano_cpus)
			let cpuLimitNano: number | undefined;
			if (cpuLimit !== null) {
				cpuLimitNano = Math.round(cpuLimit * 1e9);
			}

			const parsedEnv = { ...envVars };
			const effectiveProjectId = selectedProjectId || undefined;

			// Map sourceSubType to the API source_type value
			const sourceTypeMap: Record<SourceSubType, string> = {
				'git-public': 'git',
				'git-github-app': 'git',
				dockerfile: 'dockerfile',
				compose: 'compose',
				image: 'image'
			};
			const apiSourceType = sourceTypeMap[sourceSubType];
			const deployBranch = (branch.trim() || 'main').trim();

			const appData: Parameters<typeof createApp>[0] = {
				name: name.trim(),
				source_type: apiSourceType,
				container_port: containerPort,
				...(apiSourceType === 'git' ? { build_method: buildMethod } : {}),
				...(sourceUrl.trim() ? { source_url: sourceUrl.trim() } : {}),
				...(domain.trim() && !isWorker ? { domain: domain.trim() } : {}),
				readiness,
				...(isWorker ? { ready_after_secs: readyAfterSecs } : {}),
				...(effectiveProjectId ? { project_id: effectiveProjectId } : {}),
				...(isGitSource
					? { branch: deployBranch, auto_deploy: autoDeploy, watch_branch: deployBranch }
					: {}),
				...(healthCheckPath.trim() && readiness === 'http'
					? { health_check_path: healthCheckPath.trim() }
					: {}),
				health_check_interval: healthCheckInterval,
				health_failure_threshold: healthFailureThreshold,
				drain_timeout: drainTimeout,
				...(memoryLimitBytes ? { memory_limit: memoryLimitBytes } : {}),
				...(cpuLimitNano ? { cpu_limit: cpuLimitNano } : {}),
				...(Object.keys(parsedEnv).length > 0 ? { env_vars: parsedEnv } : {}),
				...(volumes.length > 0 ? { volumes } : {}),
				...(registryCredentialId ? { registry_credential_id: registryCredentialId } : {}),
				...(githubAppId ? { github_app_id: githubAppId } : {}),
				...(sourceSubType === 'dockerfile' ? { dockerfile_content: dockerfileContent } : {}),
				...(sourceSubType === 'compose' ? { compose_content: composeContent } : {}),
				...(baseDir.trim() ? { base_dir: baseDir.trim() } : {}),
				...(staticBuild ? { static_build: true } : {})
			};

			createStage = 'Creating the app…';
			const app = await createApp(appData);

			// Start the first deployment and follow it live.
			createStage = 'Starting the first deploy…';
			let started: DeployStarted;
			try {
				started = await deployApp(app.id);
			} catch {
				// The app exists and is fine; only the deploy request failed.
				await goto(`/apps/${app.id}`);
				return;
			}
			await goto(`/apps/${app.id}/deployments/${started.deployment_id}`);
		} catch (e: any) {
			error = e.message ?? 'Failed to create app';
			loading = false;
			createStage = '';
		}
	}
</script>

<PageTitle title="New app" />

<div class="min-h-screen space-y-6 pb-12">
	<!-- Header -->
	<div class="flex items-center gap-4">
		<Breadcrumb items={[{ label: 'Apps', href: '/apps' }, { label: 'New app' }]} />
	</div>
	<h1 class="text-2xl font-extrabold sm:text-3xl">New app</h1>

	{#if error}
		<ErrorAlert title="Configuration Error" description={error} />
	{/if}

	<!-- App identity — always visible -->
	<div class="grid gap-4 sm:grid-cols-2">
		<div class="space-y-2">
			<Label for="app-name">Application Name *</Label>
			<Input
				id="app-name"
				type="text"
				value={name}
				oninput={handleNameInput}
				placeholder="my-app"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">Lowercase letters, numbers, and hyphens only</p>
		</div>
		<div class="space-y-2">
			<Label>Project</Label>
			<Select.Root type="single" bind:value={selectedProjectId}>
				<Select.Trigger>
					{selectedProjectId
						? (projects.find((p) => p.id === selectedProjectId)?.name ?? 'Select project')
						: 'No project'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">No project</Select.Item>
					{#each projects as project (project.id)}
						<Select.Item value={project.id}>{project.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
	</div>

	<!-- Progress Steps -->
	<AppWizardSteps
		steps={visibleSteps}
		currentStep={visibleStepIndex}
		onStepClick={(index) => {
			const real = toRealStep(index);
			if (real < currentStep) currentStep = real;
		}}
	/>

	<!-- Step Content -->
	<Card.Root class="border-border bg-card">
		<Card.Header>
			<Card.Title class="font-mono text-lg tracking-wider uppercase">
				{steps[currentStep].label}
			</Card.Title>
			<Card.Description>
				{#if currentStep === 0}
					Configure where your application code comes from
				{:else if currentStep === 1}
					Choose how your application is built and deployed
				{:else if currentStep === 2}
					Set up networking, domain, and health checks
				{:else if currentStep === 3}
					Configure resource limits and persistent storage
				{:else if currentStep === 4}
					Define environment variables and registry credentials
				{:else}
					Review your configuration before deployment
				{/if}
			</Card.Description>
		</Card.Header>
		<Card.Content>
			{#if currentStep === 0}
				<AppWizardSource
					bind:name
					{nameManuallyEdited}
					bind:sourceSubType
					bind:sourceUrl
					bind:githubAppId
					bind:branch
					bind:dockerfileContent
					bind:composeContent
				/>
			{:else if currentStep === 1}
				<AppWizardBuild
					bind:buildMethod
					bind:autoDeploy
					bind:branch
					bind:baseDir
					bind:staticBuild
					{sourceSubType}
					{sourceUrl}
					{githubAppId}
				/>
			{:else if currentStep === 2}
				<AppWizardNetwork
					bind:domain
					appName={name}
					{appsDomain}
					{dashboardDomain}
					bind:healthCheckPath
					bind:healthCheckInterval
					bind:healthFailureThreshold
					bind:drainTimeout
					bind:containerPort
					bind:portTouched
					bind:readiness
					bind:readyAfterSecs
					{sourceSubType}
					{sourceUrl}
					{dockerfileContent}
					{composeContent}
					{registryCredentialId}
					{buildMethod}
				/>
			{:else if currentStep === 3}
				<AppWizardResources
					{hostMemoryBytes}
					appName={name}
					canEditVolumes={isAdmin()}
					bind:memoryLimit
					bind:memoryUnit
					bind:cpuLimit
					bind:volumes
					bind:newVolumePath
					onAddVolume={addVolume}
					onRemoveVolume={removeVolume}
				/>
			{:else if currentStep === 4}
				<AppWizardEnv
					bind:envVars
					bind:registryCredentialId
					{sourceSubType}
					{registryCredentials}
				/>
			{:else}
				<AppWizardReview
					{name}
					{selectedProjectId}
					{sourceSubType}
					{sourceUrl}
					{githubAppId}
					{dockerfileContent}
					{composeContent}
					{buildMethod}
					{containerPort}
					{readiness}
					{readyAfterSecs}
					{autoDeploy}
					branch={isGitSource ? branch : ''}
					{baseDir}
					{staticBuild}
					{domain}
					{appsDomain}
					{healthCheckPath}
					{healthCheckInterval}
					{healthFailureThreshold}
					{drainTimeout}
					{memoryLimit}
					{memoryUnit}
					{cpuLimit}
					{volumes}
					{envVars}
					{registryCredentialId}
					{projects}
					{registryCredentials}
				/>
			{/if}
		</Card.Content>
		<Card.Footer class="flex justify-between">
			<div>
				{#if currentStep > 0}
					<Button variant="outline" onclick={prevStep} disabled={loading}>
						<ChevronLeft class="mr-2 h-4 w-4" />
						Previous
					</Button>
				{/if}
			</div>
			<div class="flex gap-2">
				{#if currentStep < steps.length - 1}
					<Button onclick={nextStep}>
						Next
						<ChevronRight class="ml-2 h-4 w-4" />
					</Button>
				{:else}
					<Button
						onclick={handleCreate}
						disabled={loading}
						variant="outline"
						class="border-success text-success"
					>
						{#if loading}
							<span class="animate-pulse">{createStage || 'Deploying…'}</span>
						{:else}
							<Rocket class="mr-2 h-4 w-4" />
							Deploy Application
						{/if}
					</Button>
				{/if}
			</div>
		</Card.Footer>
	</Card.Root>
</div>
