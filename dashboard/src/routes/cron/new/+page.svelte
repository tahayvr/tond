<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createCronJob, type CronVolumeInput, type Project } from '$lib/api';
	import CronVolumeFields, {
		OWN_FOLDER,
		cronVolumeLabel,
		validateCronVolume
	} from '$lib/components/forms/cron-volume-fields.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import ErrorAlert from '$lib/components/alerts/error-alert.svelte';
	import { isAdmin } from '$lib/auth.svelte';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import Clock from '~icons/ri/time-line';
	import Plus from '~icons/ri/add-line';
	import Trash2 from '~icons/ri/delete-bin-line';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const projectIdFromQuery = $derived(page.url.searchParams.get('project_id'));

	let loading = $state(false);
	let error = $state('');
	let projects = $derived(data.projects as Project[]);

	let name = $state('');
	let schedule = $state('');
	let image = $state('');
	let command = $state('');
	let selectedProjectId = $state('');
	let timeoutSeconds = $state('');

	// New fields
	let timezone = $state('');
	let concurrencyPolicy = $state('skip');
	let memoryMb = $state('');
	let cpuShares = $state('');
	let maxRetries = $state('0');
	let retryDelaySeconds = $state('0');

	// Environment variables
	let envVars = $state<Record<string, string>>({});
	let envDialogOpen = $state(false);
	let envEditingKey = $state<string | null>(null);
	let envNewKey = $state('');
	let envNewValue = $state('');
	let envDialogError = $state('');

	let envEntries = $derived(Object.entries(envVars));

	// Volume mounts
	// Host folders are chosen by the server: the job's own folder or an app's volume.
	let volumes = $state<CronVolumeInput[]>([]);
	let volDialogOpen = $state(false);
	let volEditingIndex = $state<number | null>(null);
	let volSource = $state(OWN_FOLDER);
	let volName = $state('');
	let volContainerPath = $state('');
	let volDialogError = $state('');

	// Common schedule presets
	const PRESETS = [
		{ label: 'Every minute', value: '* * * * *' },
		{ label: 'Every 5 minutes', value: '*/5 * * * *' },
		{ label: 'Every 15 minutes', value: '*/15 * * * *' },
		{ label: 'Every hour', value: '0 * * * *' },
		{ label: 'Every 6 hours', value: '0 */6 * * *' },
		{ label: 'Every day at midnight', value: '0 0 * * *' },
		{ label: 'Every day at 2am', value: '0 2 * * *' },
		{ label: 'Every Sunday at midnight', value: '0 0 * * 0' }
	];

	$effect(() => {
		setUiContext(projectsContext());
		if (projectIdFromQuery) {
			selectedProjectId = projectIdFromQuery;
		}
		return () => clearUiContext();
	});

	async function handleCreate() {
		if (!name.trim()) {
			error = 'Job name is required';
			return;
		}
		if (!schedule.trim()) {
			error = 'Schedule is required';
			return;
		}
		if (!image.trim()) {
			error = 'Docker image is required';
			return;
		}
		error = '';
		loading = true;
		try {
			const commandArr = command.trim()
				? (command
						.trim()
						.match(/(?:[^\s"]+|"[^"]*")+/g)
						?.map((t) => t.replace(/^"|"$/g, '')) ?? [])
				: [];
			const timeoutNum = timeoutSeconds.trim() ? parseInt(timeoutSeconds.trim(), 10) : null;
			const hasEnv = Object.keys(envVars).length > 0;
			const memoryNum = memoryMb.trim() ? parseInt(memoryMb.trim(), 10) : null;
			const cpuNum = cpuShares.trim() ? parseInt(cpuShares.trim(), 10) : null;
			const retriesNum = parseInt(maxRetries, 10) || 0;
			const retryDelayNum = parseInt(retryDelaySeconds, 10) || 0;
			const created = await createCronJob({
				name: name.trim(),
				schedule: schedule.trim(),
				image: image.trim(),
				...(commandArr.length > 0 ? { command: commandArr } : {}),
				...(selectedProjectId ? { project_id: selectedProjectId } : {}),
				...(timeoutNum !== null ? { timeout_seconds: timeoutNum } : {}),
				...(hasEnv ? { env: envVars } : {}),
				concurrency_policy: concurrencyPolicy,
				...(memoryNum !== null ? { memory_mb: memoryNum } : {}),
				...(cpuNum !== null ? { cpu_shares: cpuNum } : {}),
				...(timezone.trim() ? { timezone: timezone.trim() } : {}),
				...(retriesNum > 0 ? { max_retries: retriesNum } : {}),
				...(retryDelayNum > 0 ? { retry_delay_seconds: retryDelayNum } : {}),
				...(volumes.length > 0 ? { volumes } : {})
			});
			goto(`/cron/${created.id}`);
		} catch (e: any) {
			error = e.message ?? 'Failed to create cron job';
			loading = false;
		}
	}

	function applyPreset(value: string) {
		schedule = value;
	}

	function openEnvAdd() {
		envEditingKey = null;
		envNewKey = '';
		envNewValue = '';
		envDialogError = '';
		envDialogOpen = true;
	}

	function handleEnvConfirm() {
		const key = envNewKey.trim();
		if (!key) {
			envDialogError = 'Key is required';
			return;
		}
		if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
			envDialogError =
				'Key must start with a letter or underscore and contain only letters, numbers, and underscores';
			return;
		}
		if (envEditingKey !== null && envEditingKey !== key) {
			const next = { ...envVars };
			delete next[envEditingKey];
			envVars = { ...next, [key]: envNewValue };
		} else {
			envVars = { ...envVars, [key]: envNewValue };
		}
		envDialogOpen = false;
	}

	function handleEnvRemove(key: string) {
		const next = { ...envVars };
		delete next[key];
		envVars = next;
	}

	function handleEnvKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') handleEnvConfirm();
	}

	// Volume handlers
	function openVolAdd() {
		volEditingIndex = null;
		volSource = OWN_FOLDER;
		volName = '';
		volContainerPath = '';
		volDialogError = '';
		volDialogOpen = true;
	}

	function openVolEdit(index: number) {
		volEditingIndex = index;
		volSource = volumes[index].app || OWN_FOLDER;
		volName = volumes[index].name ?? '';
		volContainerPath = volumes[index].container_path;
		volDialogError = '';
		volDialogOpen = true;
	}

	function handleVolConfirm() {
		const name = volName.trim();
		const container = volContainerPath.trim();
		volDialogError = validateCronVolume(name, container);
		if (volDialogError) return;
		const entry: CronVolumeInput = {
			container_path: container,
			...(name ? { name } : {}),
			...(volSource !== OWN_FOLDER ? { app: volSource } : {})
		};
		if (volEditingIndex !== null) {
			volumes = volumes.map((v, i) => (i === volEditingIndex ? entry : v));
		} else {
			volumes = [...volumes, entry];
		}
		volDialogOpen = false;
	}

	function handleVolRemove(index: number) {
		volumes = volumes.filter((_, i) => i !== index);
	}

	function handleVolKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') handleVolConfirm();
	}
</script>

<div class="space-y-6 pb-12">
	<Breadcrumb items={[{ label: 'Cron Jobs', href: '/cron' }, { label: 'New Cron Job' }]} />

	{#if error}
		<ErrorAlert title="Error" description={error} />
	{/if}

	{#if !isAdmin()}
		<div class="rounded-md border bg-muted/30 p-3 text-sm text-muted-foreground">
			Only admins can create or change cron jobs, because a job runs any image and command on this
			server.
		</div>
	{/if}

	<div class="space-y-1">
		<h2 class="text-3xl font-extrabold uppercase">New Cron Job</h2>
		<p class="text-sm text-muted-foreground">Schedule a recurring Docker container task.</p>
	</div>

	<div class="grid max-w-xl gap-5">
		<!-- Name -->
		<div class="grid gap-2">
			<Label for="name">Name</Label>
			<Input id="name" type="text" bind:value={name} placeholder="nightly-backup" />
			<p class="text-xs text-muted-foreground">A short identifier. Lowercase, no spaces.</p>
		</div>

		<!-- Schedule -->
		<div class="grid gap-2">
			<Label for="schedule">Schedule</Label>
			<div class="flex gap-2">
				<Input
					id="schedule"
					type="text"
					bind:value={schedule}
					placeholder="0 2 * * *"
					class="flex-1 font-mono"
				/>
				<Select.Root
					type="single"
					value=""
					onValueChange={(v) => {
						if (v) applyPreset(v);
					}}
				>
					<Select.Trigger class="w-40 text-xs">Presets</Select.Trigger>
					<Select.Content>
						{#each PRESETS as preset (preset.value)}
							<Select.Item value={preset.value} class="text-xs">
								<span>{preset.label}</span>
								<span class="ml-2 font-mono text-muted-foreground">{preset.value}</span>
							</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
			<p class="text-xs text-muted-foreground">
				5-field cron expression (minute hour day-of-month month day-of-week). Pick a preset or type
				your own.
			</p>
		</div>

		<!-- Timezone -->
		<div class="grid gap-2">
			<Label for="timezone">Timezone <span class="text-muted-foreground">(optional)</span></Label>
			<Input
				id="timezone"
				type="text"
				bind:value={timezone}
				placeholder="America/New_York"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">
				IANA timezone for schedule evaluation (e.g. <code class="font-mono">Europe/London</code>).
				Leave empty to use UTC.
			</p>
		</div>

		<!-- Image -->
		<div class="grid gap-2">
			<Label for="image">Docker Image</Label>
			<Input
				id="image"
				type="text"
				bind:value={image}
				placeholder="alpine:latest"
				class="font-mono"
			/>
		</div>

		<!-- Command -->
		<div class="grid gap-2">
			<Label for="command">Command <span class="text-muted-foreground">(optional)</span></Label>
			<Input
				id="command"
				type="text"
				bind:value={command}
				placeholder="sh -c &quot;echo hello world&quot;"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">
				Space-separated. Quote arguments that contain spaces. Leave empty to use the image default
				entrypoint.
			</p>
		</div>

		<!-- Timeout -->
		<div class="grid gap-2">
			<Label for="timeout">
				Timeout <span class="text-muted-foreground">(optional, seconds)</span>
			</Label>
			<Input
				id="timeout"
				type="number"
				min="1"
				bind:value={timeoutSeconds}
				placeholder="300"
				class="font-mono"
			/>
			<p class="text-xs text-muted-foreground">
				Maximum time the container is allowed to run. Leave empty for no timeout.
			</p>
		</div>

		<!-- Concurrency Policy -->
		<div class="grid gap-2">
			<Label>Concurrency Policy</Label>
			<Select.Root type="single" bind:value={concurrencyPolicy}>
				<Select.Trigger class="w-full">
					{concurrencyPolicy === 'allow' ? 'Allow concurrent runs' : 'Skip if already running'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="skip">Skip if already running</Select.Item>
					<Select.Item value="allow">Allow concurrent runs</Select.Item>
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">
				Whether a new run can start while a previous run is still active.
			</p>
		</div>

		<!-- Resource Limits -->
		<div class="grid grid-cols-2 gap-4">
			<div class="grid gap-2">
				<Label for="memory-mb">
					Memory <span class="text-muted-foreground">(MiB, optional)</span>
				</Label>
				<Input
					id="memory-mb"
					type="number"
					min="1"
					bind:value={memoryMb}
					placeholder="512"
					class="font-mono"
				/>
			</div>
			<div class="grid gap-2">
				<Label for="cpu-shares">
					CPU Shares <span class="text-muted-foreground">(optional)</span>
				</Label>
				<Input
					id="cpu-shares"
					type="number"
					min="1"
					bind:value={cpuShares}
					placeholder="1024"
					class="font-mono"
				/>
			</div>
		</div>
		<p class="-mt-3 text-xs text-muted-foreground">
			Leave empty for no limits. CPU shares are a relative weight (default 1024).
		</p>

		<!-- Retry Policy -->
		<div class="grid grid-cols-2 gap-4">
			<div class="grid gap-2">
				<Label for="max-retries">Max Retries</Label>
				<Input
					id="max-retries"
					type="number"
					min="0"
					bind:value={maxRetries}
					placeholder="0"
					class="font-mono"
				/>
			</div>
			<div class="grid gap-2">
				<Label for="retry-delay"
					>Retry Delay <span class="text-muted-foreground">(seconds)</span></Label
				>
				<Input
					id="retry-delay"
					type="number"
					min="0"
					bind:value={retryDelaySeconds}
					placeholder="0"
					class="font-mono"
				/>
			</div>
		</div>
		<p class="-mt-3 text-xs text-muted-foreground">
			Number of automatic retries on non-zero exit. Set to 0 to disable.
		</p>

		<!-- Volume Mounts -->
		<div class="grid gap-2">
			<div class="flex items-center justify-between">
				<Label>Volume Mounts <span class="text-muted-foreground">(optional)</span></Label>
				<Dialog.Root bind:open={volDialogOpen}>
					<Dialog.Trigger>
						{#snippet child({ props })}
							<Button variant="outline" size="sm" onclick={openVolAdd} {...props}>
								<Plus class="mr-1 h-3.5 w-3.5" />
								Add
							</Button>
						{/snippet}
					</Dialog.Trigger>
					<Dialog.Content class="sm:max-w-md">
						<Dialog.Header>
							<Dialog.Title>
								{volEditingIndex !== null ? 'Edit' : 'Add'} Volume Mount
							</Dialog.Title>
							<Dialog.Description
								>Mount a persistent folder into the job's container.</Dialog.Description
							>
						</Dialog.Header>
						<CronVolumeFields
							bind:source={volSource}
							bind:name={volName}
							bind:containerPath={volContainerPath}
							onkeydown={handleVolKeydown}
						/>
						{#if volDialogError}
							<p class="text-xs text-destructive">{volDialogError}</p>
						{/if}
						<Dialog.Footer>
							<Button variant="outline" onclick={() => (volDialogOpen = false)}>Cancel</Button>
							<Button onclick={handleVolConfirm}>
								{volEditingIndex !== null ? 'Save' : 'Add'}
							</Button>
						</Dialog.Footer>
					</Dialog.Content>
				</Dialog.Root>
			</div>

			{#if volumes.length === 0}
				<div class="rounded border border-dashed border-border py-4 text-center">
					<p class="text-xs text-muted-foreground">No volume mounts.</p>
				</div>
			{:else}
				<div class="overflow-hidden rounded border border-border">
					<table class="w-full text-sm">
						<tbody>
							{#each volumes as vol, i (i)}
								<tr class={i < volumes.length - 1 ? 'border-b border-border' : ''}>
									<td class="px-3 py-1.5 font-mono text-xs">{cronVolumeLabel(vol)}</td>
									<td class="px-3 py-1.5 text-center text-xs text-muted-foreground">→</td>
									<td class="px-3 py-1.5 font-mono text-xs">{vol.container_path}</td>
									<td class="flex gap-1 px-2 py-1.5">
										<Button
											variant="ghost"
											size="icon"
											class="h-6 w-6 text-muted-foreground"
											aria-label="Edit volume"
											onclick={() => openVolEdit(i)}
										>
											<Plus class="h-3 w-3" />
										</Button>
										<Button
											variant="ghost"
											size="icon"
											class="h-6 w-6 text-muted-foreground hover:text-destructive"
											aria-label="Remove volume"
											onclick={() => handleVolRemove(i)}
										>
											<Trash2 class="h-3 w-3" />
										</Button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>

		<!-- Environment Variables -->
		<div class="grid gap-2">
			<div class="flex items-center justify-between">
				<Label>Environment Variables <span class="text-muted-foreground">(optional)</span></Label>
				<Dialog.Root bind:open={envDialogOpen}>
					<Dialog.Trigger>
						{#snippet child({ props })}
							<Button variant="outline" size="sm" onclick={openEnvAdd} {...props}>
								<Plus class="mr-1 h-3.5 w-3.5" />
								Add
							</Button>
						{/snippet}
					</Dialog.Trigger>
					<Dialog.Content class="sm:max-w-md">
						<Dialog.Header>
							<Dialog.Title>
								{envEditingKey !== null ? 'Edit' : 'Add'} Environment Variable
							</Dialog.Title>
							<Dialog.Description>
								{envEditingKey !== null
									? 'Update the key or value for this environment variable.'
									: 'Enter a key and value for the new environment variable.'}
							</Dialog.Description>
						</Dialog.Header>
						<div class="space-y-4 py-2">
							<div class="space-y-2">
								<Label for="env-key">Key</Label>
								<Input
									id="env-key"
									bind:value={envNewKey}
									placeholder="DATABASE_URL"
									class="font-mono"
									onkeydown={handleEnvKeydown}
									autofocus
								/>
							</div>
							<div class="space-y-2">
								<Label for="env-value">Value</Label>
								<Input
									id="env-value"
									bind:value={envNewValue}
									placeholder="postgres://user:pass@host/db"
									class="font-mono"
									onkeydown={handleEnvKeydown}
								/>
							</div>
							{#if envDialogError}
								<p class="text-xs text-destructive">{envDialogError}</p>
							{/if}
						</div>
						<Dialog.Footer>
							<Button variant="outline" onclick={() => (envDialogOpen = false)}>Cancel</Button>
							<Button onclick={handleEnvConfirm}>{envEditingKey !== null ? 'Save' : 'Add'}</Button>
						</Dialog.Footer>
					</Dialog.Content>
				</Dialog.Root>
			</div>

			{#if envEntries.length === 0}
				<div class="rounded border border-dashed border-border py-4 text-center">
					<p class="text-xs text-muted-foreground">No environment variables.</p>
				</div>
			{:else}
				<div class="overflow-hidden rounded border border-border">
					<table class="w-full text-sm">
						<tbody>
							{#each envEntries as [key, value], i (key)}
								<tr class={i < envEntries.length - 1 ? 'border-b border-border' : ''}>
									<td class="px-3 py-1.5 font-mono text-xs font-medium">{key}</td>
									<td class="max-w-0 truncate px-3 py-1.5 font-mono text-xs text-muted-foreground">
										{#if value}
											{value}
										{:else}
											<span class="italic">empty</span>
										{/if}
									</td>
									<td class="px-2 py-1.5">
										<Button
											variant="ghost"
											size="icon"
											class="h-6 w-6 text-muted-foreground hover:text-destructive"
											aria-label="Remove variable"
											onclick={() => handleEnvRemove(key)}
										>
											<Trash2 class="h-3 w-3" />
										</Button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</div>

		<!-- Project (optional) -->
		{#if projects.length > 0}
			<div class="grid gap-2">
				<Label>Project <span class="text-muted-foreground">(optional)</span></Label>
				<Select.Root
					type="single"
					value={selectedProjectId || '__none__'}
					onValueChange={(v) => {
						selectedProjectId = v === '__none__' ? '' : v;
					}}
				>
					<Select.Trigger class="w-full">
						{#if selectedProjectId}
							{projects.find((p) => p.id === selectedProjectId)?.name ?? selectedProjectId}
						{:else}
							No project
						{/if}
					</Select.Trigger>
					<Select.Content>
						<Select.Item value="__none__">No project</Select.Item>
						{#each projects as proj (proj.id)}
							<Select.Item value={proj.id}>{proj.name}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<p class="text-xs text-muted-foreground">Associate this job with a project (optional).</p>
			</div>
		{/if}
	</div>

	<div>
		<Button
			onclick={handleCreate}
			disabled={loading || !isAdmin()}
			variant="outline"
			class="border-success text-success"
		>
			{#if loading}
				<span class="animate-pulse">Creating...</span>
			{:else}
				<Clock class="mr-2 h-4 w-4" />
				Create Cron Job
			{/if}
		</Button>
	</div>
</div>
