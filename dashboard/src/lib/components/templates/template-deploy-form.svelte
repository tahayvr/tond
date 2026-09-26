<!--
  TemplateDeployForm — name, address, inputs and the memory check for a
  template deploy. Calls `ondeployed` with the server's result (which holds
  the one-time credentials).
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		ApiError,
		deployTemplate,
		type Project,
		type Template,
		type TemplateDeployResult,
		type TemplateHostMemory
	} from '$lib/api';
	import { can } from '$lib/auth.svelte';
	import {
		appHost,
		appNameError,
		formatRam,
		inputError,
		inputsToSend,
		memoryFit,
		memoryWarning
	} from '$lib/templates';
	import { cn } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import RocketIcon from '~icons/ri/rocket-2-line';
	import AlertIcon from '~icons/ri/alert-line';
	import KeyIcon from '~icons/ri/key-2-line';
	import ArrowDown from '~icons/ri/arrow-down-s-line';

	let {
		template,
		memory,
		appsDomain,
		projects,
		ondeployed
	}: {
		template: Template;
		memory: TemplateHostMemory | null;
		appsDomain: string;
		projects: Project[];
		ondeployed: (result: TemplateDeployResult) => void;
	} = $props();

	// The form starts from the template (the page re-mounts it per template).
	let name = $state(untrack(() => template.id));
	let values = $state<Record<string, string>>(
		untrack(() => Object.fromEntries(template.inputs.map((i) => [i.key, i.default ?? ''])))
	);
	let domain = $state('');
	let projectId = $state('');
	let allowLowMemory = $state(false);
	let showGenerated = $state(false);
	let submitted = $state(false);
	let pending = $state(false);
	let error = $state('');

	let asked = $derived(template.inputs.filter((i) => !i.generated));
	let generated = $derived(template.inputs.filter((i) => i.generated));
	let host = $derived(appHost(name, domain, appsDomain));
	let fit = $derived(memoryFit(template, memory));
	let warning = $derived(memoryWarning(template, memory));
	let nameProblem = $derived(appNameError(name));
	let inputProblems = $derived(
		Object.fromEntries(
			template.inputs
				.map((i) => [i.key, inputError(i, values[i.key] ?? '')] as const)
				.filter(([, e]) => e !== null)
		) as Record<string, string>
	);
	let blocked = $derived(fit === 'short' && !allowLowMemory);
	let allowed = $derived(can('apps:write') && can('apps:deploy') && can('settings:write'));

	async function handleSubmit(event: SubmitEvent) {
		event.preventDefault();
		submitted = true;
		if (pending || nameProblem || Object.keys(inputProblems).length > 0 || blocked) return;
		pending = true;
		error = '';
		try {
			const result = await deployTemplate(template.id, {
				name,
				...(domain.trim() ? { domain: domain.trim() } : {}),
				...(projectId ? { project_id: projectId } : {}),
				inputs: inputsToSend(template.inputs, values),
				allow_low_memory: allowLowMemory
			});
			ondeployed(result);
		} catch (e: any) {
			if (e instanceof ApiError && e.code === 'insufficient_memory') {
				// Free memory changed since the page loaded: ask again.
				allowLowMemory = false;
			}
			error = e.message ?? 'The deploy could not be started. Try again.';
		} finally {
			pending = false;
		}
	}
</script>

<form class="space-y-5" onsubmit={handleSubmit} novalidate>
	<div class="space-y-1.5">
		<Label for="tpl-name">App name</Label>
		<Input
			id="tpl-name"
			bind:value={name}
			autocomplete="off"
			spellcheck={false}
			aria-invalid={submitted && !!nameProblem}
			aria-describedby="tpl-name-help"
		/>
		<p
			id="tpl-name-help"
			class={cn('text-xs', submitted && nameProblem ? 'text-destructive' : 'text-muted-foreground')}
		>
			{submitted && nameProblem ? nameProblem : 'Lowercase letters, digits and hyphens.'}
		</p>
	</div>

	<div class="space-y-1.5">
		<Label for="tpl-domain"
			>Domain <span class="font-normal text-muted-foreground">(optional)</span></Label
		>
		<Input
			id="tpl-domain"
			bind:value={domain}
			placeholder={appsDomain ? `${name || 'app'}.${appsDomain}` : 'app.example.com'}
			autocomplete="off"
			spellcheck={false}
			aria-describedby="tpl-domain-help"
		/>
		<p id="tpl-domain-help" class="text-xs text-muted-foreground">
			{#if domain.trim()}
				Point this domain's DNS at the server; Tond gets the certificate.
			{:else if host}
				Leave empty to use <span class="font-mono">{host}</span>.
			{:else}
				No apps domain is set, so add a domain here or later on the app's page. The app needs one to
				be reachable over HTTPS.
			{/if}
		</p>
	</div>

	{#if projects.length > 0}
		<div class="space-y-1.5">
			<Label for="tpl-project"
				>Project <span class="font-normal text-muted-foreground">(optional)</span></Label
			>
			<Select.Root type="single" bind:value={projectId}>
				<Select.Trigger id="tpl-project" class="w-full">
					{projectId
						? (projects.find((p) => p.id === projectId)?.name ?? 'No project')
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
	{/if}

	{#each asked as input (input.key)}
		<div class="space-y-1.5">
			<Label for="tpl-in-{input.key}">
				{input.label}
				{#if !input.required}<span class="font-normal text-muted-foreground">(optional)</span>{/if}
			</Label>
			<Input
				id="tpl-in-{input.key}"
				type={input.kind === 'password' ? 'password' : input.kind === 'email' ? 'email' : 'text'}
				bind:value={values[input.key]}
				autocomplete="off"
				spellcheck={false}
				aria-invalid={submitted && !!inputProblems[input.key]}
				aria-describedby="tpl-in-{input.key}-help"
			/>
			<p
				id="tpl-in-{input.key}-help"
				class={cn(
					'text-xs',
					submitted && inputProblems[input.key] ? 'text-destructive' : 'text-muted-foreground'
				)}
			>
				{#if submitted && inputProblems[input.key]}
					{inputProblems[input.key]}
				{:else}
					{input.description ?? ''}
					{#if input.credential}You'll see it again right after the deploy.{/if}
				{/if}
			</p>
		</div>
	{/each}

	{#if generated.length > 0}
		<div class="rounded-md border bg-muted/30">
			<button
				type="button"
				class="flex w-full items-center gap-2 px-3 py-2.5 text-left text-sm"
				aria-expanded={showGenerated}
				aria-controls="tpl-generated"
				onclick={() => (showGenerated = !showGenerated)}
			>
				<KeyIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
				<span class="flex-1">
					{generated.length === 1 ? '1 secret is' : `${generated.length} secrets are`} generated for you
					<span class="block text-xs text-muted-foreground"
						>Stored encrypted. Passwords you need to sign in are shown once after the deploy.</span
					>
				</span>
				<ArrowDown
					class={cn('size-4 shrink-0 transition-transform', showGenerated && 'rotate-180')}
					aria-hidden="true"
				/>
			</button>
			{#if showGenerated}
				<div id="tpl-generated" class="space-y-4 border-t px-3 py-3">
					<p class="text-xs text-muted-foreground">Leave a field empty to generate it.</p>
					{#each generated as input (input.key)}
						<div class="space-y-1.5">
							<Label for="tpl-in-{input.key}">{input.label}</Label>
							<Input
								id="tpl-in-{input.key}"
								type="password"
								placeholder="Generated"
								bind:value={values[input.key]}
								autocomplete="new-password"
								aria-invalid={submitted && !!inputProblems[input.key]}
							/>
							{#if submitted && inputProblems[input.key]}
								<p class="text-xs text-destructive">{inputProblems[input.key]}</p>
							{:else if input.description}
								<p class="text-xs text-muted-foreground">{input.description}</p>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/if}

	{#if warning}
		<div
			class={cn(
				'space-y-2 rounded-md border px-3 py-2.5 text-sm',
				fit === 'short'
					? 'border-destructive/40 bg-destructive/10 text-destructive'
					: 'border-warn/40 bg-warn/10 text-warn'
			)}
			role={fit === 'short' ? 'alert' : 'status'}
		>
			<p class="flex items-start gap-2">
				<AlertIcon class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
				<span>{warning}</span>
			</p>
			{#if fit === 'short'}
				<div class="flex items-center gap-2 pl-6">
					<Checkbox id="tpl-low-memory" bind:checked={allowLowMemory} />
					<Label for="tpl-low-memory" class="text-sm font-normal">Deploy anyway</Label>
				</div>
			{/if}
		</div>
	{:else if memory}
		<p class="text-xs text-muted-foreground">
			Needs about {formatRam(template.ram_mb)} of the {formatRam(memory.available_mb)} free on this server.
		</p>
	{/if}

	<InlineError message={error} ondismiss={() => (error = '')} />

	{#if allowed}
		<Button type="submit" class="w-full" disabled={pending || blocked}>
			<RocketIcon class="mr-1 size-4" aria-hidden="true" />
			{pending ? 'Starting the deploy…' : `Deploy ${template.name}`}
		</Button>
	{:else}
		<p class="rounded-md border bg-muted/40 px-3 py-2.5 text-sm text-muted-foreground">
			Deploying a template creates a Docker Compose app, which needs an admin role. Ask an admin to
			deploy it.
		</p>
	{/if}
</form>
