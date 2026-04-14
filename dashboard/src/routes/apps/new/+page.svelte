<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		createApp,
		createDatabase,
		createProject,
		deployApp,
		type ManagedDatabaseDetail,
		type Project,
		type RegistryCredential,
		type TemplateCatalogEntry
	} from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { SvelteMap } from 'svelte/reactivity';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import ChevronRight from '@lucide/svelte/icons/chevron-right';
	import ChevronLeft from '@lucide/svelte/icons/chevron-left';
	import Rocket from '@lucide/svelte/icons/rocket';
	import GitBranch from '@lucide/svelte/icons/git-branch';
	import Network from '@lucide/svelte/icons/network';
	import Settings from '@lucide/svelte/icons/settings';
	import CheckCircle from '@lucide/svelte/icons/check-circle';
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
	let templates = $derived(data.templates as TemplateCatalogEntry[]);
	let selectedTemplateId = $state('');

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
	let autoDeploy = $state(false);
	let watchBranch = $state('main');
	let templateSourceRef = $state('');
	let baseDir = $state('');
	let staticBuild = $state(false);

	// Step 3: Network
	let domain = $state('');
	let healthCheckPath = $state('/health');
	let healthCheckInterval = $state(30);
	let healthFailureThreshold = $state(3);
	let drainTimeout = $state(30);

	// Step 4: Resources
	let memoryLimit = $state<number | null>(null);
	let memoryUnit = $state<'mb' | 'gb'>('mb');
	let cpuLimit = $state<number | null>(null);
	let volumes = $state<{ host_path: string; container_path: string }[]>([]);
	let newVolumePath = $state('');

	// Step 5: Environment
	let envVars = $state<Record<string, string>>({});
	let registryCredentialId = $state<string>('');

	let selectedTemplate = $derived(
		sourceSubType === 'template' && selectedTemplateId
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

	// Initialize
	$effect(() => {
		setUiContext(projectsContext());
		if (projectIdFromQuery) {
			selectedProjectId = projectIdFromQuery;
		}
		return () => clearUiContext();
	});

	function applyTemplate(templateId: string) {
		selectedTemplateId = templateId;
		const template = templates.find((entry) => entry.id === templateId);
		if (!template) {
			templateSourceRef = '';
			return;
		}

		sourceSubType = 'template';
		sourceUrl = template.source.url;
		templateSourceRef = template.source.ref ?? '';
		containerPort = template.deploy.default_port;
		healthCheckPath = template.deploy.health_check_path ?? '';
		buildMethod = template.deploy.method === 'dockerfile' ? 'dockerfile' : 'tbuild';
		if (template.source.kind === 'git') {
			watchBranch = template.source.ref ?? watchBranch ?? 'main';
		}
		volumes = template.deploy.volumes.map(
			(volume: TemplateCatalogEntry['deploy']['volumes'][number]) => ({
				host_path: `/var/lib/tond/volumes/${name || template.id}/${volume.container_path.replace(/^\//, '')}`,
				container_path: volume.container_path
			})
		);
		if (!nameManuallyEdited) {
			name = template.id;
		}
		if (!domain.trim() && template.recommended_domains.length > 0) {
			domain = template.recommended_domains[0];
		}
		envVars = Object.fromEntries(
			template.required_env
				.filter((env: TemplateCatalogEntry['required_env'][number]) => !env.value)
				.map((env: TemplateCatalogEntry['required_env'][number]) => [
					env.key,
					envVars[env.key] ?? ''
				])
		);
	}

	// Build step (index 1) is irrelevant for docker source types
	let isBuildStepSkipped = $derived.by(() => {
		if (
			sourceSubType === 'dockerfile' ||
			sourceSubType === 'compose' ||
			sourceSubType === 'image'
		) {
			return true;
		}
		if (sourceSubType !== 'template' || !selectedTemplate) {
			return false;
		}
		return selectedTemplate.source.kind !== 'git';
	});

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
		if (sourceSubType === 'template' && !selectedTemplateId) {
			error = 'Please select a template';
			return false;
		}
		if (
			selectedTemplate &&
			templateRequiredInputs.some(
				(env: TemplateCatalogEntry['required_env'][number]) => !(envVars[env.key] ?? '').trim()
			)
		) {
			const missing = templateRequiredInputs.find(
				(env: TemplateCatalogEntry['required_env'][number]) => !(envVars[env.key] ?? '').trim()
			);
			error = `${missing?.key ?? 'Template env'} is required for ${selectedTemplate.name}`;
			return false;
		}
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
		} else if (sourceSubType === 'template') {
			if (!selectedTemplate) {
				error = 'Please select a template';
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
		if (!newVolumePath.trim()) return;
		const hostPath = `/var/lib/tond/volumes/${name || 'app'}/${newVolumePath.replace(/^\//, '')}`;
		volumes = [...volumes, { host_path: hostPath, container_path: newVolumePath }];
		newVolumePath = '';
	}

	function removeVolume(index: number) {
		volumes = volumes.filter((_, i) => i !== index);
	}

	async function handleCreate() {
		error = '';
		loading = true;

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

			let parsedEnv = { ...envVars };
			let templateProjectId: string | undefined;
			const effectiveProjectId = selectedProjectId || undefined;

			if (selectedTemplate?.project) {
				const createdProject = await createProject({
					name: selectedTemplate.project.name,
					description: selectedTemplate.project.description ?? undefined
				});
				templateProjectId = createdProject.id;
				projects = [createdProject, ...projects];
			}

			if (selectedTemplate) {
				const dependencyDetails = new SvelteMap<string, ManagedDatabaseDetail>();
				for (const dependency of selectedTemplate.dependencies) {
					if (dependency.kind !== 'database' || !dependency.name || !dependency.engine) continue;
					const database = await createDatabase({
						name: dependency.name,
						db_type: dependency.engine,
						...(templateProjectId || effectiveProjectId
							? { project_id: templateProjectId ?? effectiveProjectId }
							: {})
					});
					dependencyDetails.set(dependency.name, database);
				}

				for (const env of selectedTemplate.required_env) {
					if (!env.value) continue;
					parsedEnv[env.key] = interpolateTemplateValue(env.value, dependencyDetails);
				}
			}

			// Map sourceSubType to the API source_type value
			const sourceTypeMap: Record<SourceSubType, string> = {
				'git-public': 'git',
				'git-github-app': 'git',
				dockerfile: 'dockerfile',
				compose: 'compose',
				image: 'image',
				template: selectedTemplate?.source.kind ?? 'image'
			};
			const apiSourceType = sourceTypeMap[sourceSubType];
			const isGit =
				sourceSubType === 'git-public' ||
				sourceSubType === 'git-github-app' ||
				(sourceSubType === 'template' && apiSourceType === 'git');

			const appData: Parameters<typeof createApp>[0] = {
				name: name.trim(),
				source_type: apiSourceType,
				container_port: containerPort,
				...(apiSourceType === 'git' ? { build_method: buildMethod } : {}),
				...(sourceUrl.trim() ? { source_url: sourceUrl.trim() } : {}),
				...(domain.trim() ? { domain: domain.trim() } : {}),
				...(templateProjectId || effectiveProjectId
					? { project_id: templateProjectId ?? effectiveProjectId }
					: {}),
				...(isGit
					? {
							branch: templateSourceRef || watchBranch || 'main',
							auto_deploy: autoDeploy,
							watch_branch: watchBranch || templateSourceRef || 'main'
						}
					: {}),
				...(healthCheckPath.trim() ? { health_check_path: healthCheckPath.trim() } : {}),
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

			const app = await createApp(appData);
			// Trigger the first deployment immediately — do not await so we can
			// navigate to the app page while the deploy runs in the background.
			deployApp(app.id).catch(() => {
				// Errors will appear in the feed panel on the app page.
			});
			goto(`/apps/${app.id}`);
		} catch (e: any) {
			error = e.message ?? 'Failed to create app';
			loading = false;
		}
	}

	function interpolateTemplateValue(
		value: string,
		dependencies: Pick<SvelteMap<string, ManagedDatabaseDetail>, 'get'>
	): string {
		if (!value.startsWith('${db.') || !value.endsWith('}')) {
			return value;
		}

		const reference = value.slice(5, -1);
		const [dependencyName, field] = reference.split('.');
		const dependency = dependencies.get(dependencyName);
		if (!dependency) {
			throw new Error(`Unknown template dependency: ${dependencyName}`);
		}
		if (field === 'connection_string') {
			if (!dependency.connection_string) {
				throw new Error(`Dependency ${dependencyName} has no external connection string`);
			}
			return dependency.connection_string;
		}
		if (field === 'internal_connection_string') {
			if (!dependency.internal_connection_string) {
				throw new Error(`Dependency ${dependencyName} has no internal connection string`);
			}
			return dependency.internal_connection_string;
		}
		throw new Error(`Unsupported template dependency field: ${field}`);
	}
</script>

<div class="min-h-screen space-y-6 pb-12">
	<!-- Header -->
	<div class="flex items-center gap-4">
		<Breadcrumb items={[{ label: 'Applications', href: '/apps' }, { label: 'New App' }]} />
	</div>

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
					bind:dockerfileContent
					bind:composeContent
					bind:selectedTemplateId
					bind:envVars
					{templates}
					onTemplateSelect={applyTemplate}
				/>
			{:else if currentStep === 1}
				<AppWizardBuild
					bind:buildMethod
					bind:containerPort
					bind:autoDeploy
					bind:watchBranch
					bind:baseDir
					bind:staticBuild
					{sourceSubType}
				/>
			{:else if currentStep === 2}
				<AppWizardNetwork
					bind:domain
					bind:healthCheckPath
					bind:healthCheckInterval
					bind:healthFailureThreshold
					bind:drainTimeout
				/>
			{:else if currentStep === 3}
				<AppWizardResources
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
					{autoDeploy}
					{watchBranch}
					{baseDir}
					{staticBuild}
					{domain}
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
							<span class="animate-pulse">Deploying...</span>
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
