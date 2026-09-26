<script lang="ts">
	// A docker-compose.yml editor with Tond's policy check as you type
	// (POST /compose/validate, debounced) and the services it declares. With
	// `pickWebService`, the user picks the service that gets the app's port
	// and domain.
	import { validateCompose, type ComposeServiceSpec } from '$lib/api';
	import { suggestWebService } from '$lib/compose';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import HammerIcon from '~icons/ri/hammer-line';
	import BoxIcon from '~icons/ri/box-3-line';

	interface Props {
		content: string;
		appName?: string;
		id?: string;
		disabled?: boolean;
		/** Show the web-service picker (new apps). */
		pickWebService?: boolean;
		/** The picked web service ('' = let Tond pick). */
		webService?: string;
		/** The app's port, to suggest the service that declares it. */
		containerPort?: number;
		/** Called with the problems and services after each check. */
		onChecked?: (errors: string[], services: ComposeServiceSpec[]) => void;
	}

	let {
		content = $bindable(''),
		appName = '',
		id = 'compose-content',
		disabled = false,
		pickWebService = false,
		webService = $bindable(''),
		containerPort = 80,
		onChecked
	}: Props = $props();

	let errors = $state<string[]>([]);
	let services = $state<ComposeServiceSpec[]>([]);
	let checking = $state(false);
	let checkedOnce = $state(false);

	$effect(() => {
		const text = content;
		const name = appName;
		if (!text.trim()) {
			errors = [];
			services = [];
			checkedOnce = false;
			return;
		}
		let cancelled = false;
		checking = true;
		const timer = setTimeout(async () => {
			try {
				const result = await validateCompose(text, name);
				if (cancelled) return;
				errors = result.errors;
				services = result.services ?? [];
				checkedOnce = true;
				onChecked?.(errors, services);
			} catch {
				// Advisory only: saving re-checks and reports problems.
				if (!cancelled) errors = [];
			} finally {
				if (!cancelled) checking = false;
			}
		}, 500);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	});

	let suggested = $derived(suggestWebService(services, containerPort));
	// Drop a pick that left the file.
	$effect(() => {
		if (webService && services.length && !services.some((s) => s.name === webService)) {
			webService = '';
		}
	});
</script>

<div class="space-y-2">
	<Label for={id}>docker-compose.yml</Label>
	<Textarea
		{id}
		bind:value={content}
		{disabled}
		placeholder={'services:\n  web:\n    image: nginx:alpine\n    expose: ["80"]\n  cache:\n    image: redis:7-alpine\n    volumes: ["cache-data:/data"]\nvolumes:\n  cache-data: {}'}
		class="min-h-72 font-mono text-xs"
		spellcheck={false}
		aria-describedby="{id}-help"
	/>
	<p id="{id}-help" class="text-xs text-muted-foreground">
		Tond builds or pulls every service's image, then starts them with
		<code class="rounded bg-muted px-1 font-mono">docker compose up</code>. Privileged mode, host
		namespaces, extra capabilities, devices and bind mounts outside the app's folder are refused —
		use relative binds like <code class="font-mono">./data:/data</code> or named volumes (both are
		included in snapshots). Ports are bound to 127.0.0.1; give a service a domain to reach it. Write
		<code class="font-mono">${'{'}NAME}</code> to use one of the app's environment variables (secrets
		stay out of the file).
	</p>

	{#if errors.length > 0}
		<div
			class="rounded-sm border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive"
			role="alert"
		>
			<p class="font-medium">
				Tond will refuse this file ({errors.length} problem{errors.length === 1 ? '' : 's'}):
			</p>
			<ul class="mt-1 list-disc space-y-0.5 pl-4">
				{#each errors as problem (problem)}
					<li>{problem}</li>
				{/each}
			</ul>
		</div>
	{:else if checkedOnce && !checking && services.length > 0}
		<div class="space-y-2 rounded-sm border p-3" aria-live="polite">
			<p class="flex items-center gap-1.5 text-xs text-success">
				<CheckIcon class="size-3.5" aria-hidden="true" /> Looks good: {services.length} service{services.length ===
				1
					? ''
					: 's'}
			</p>
			<ul class="flex flex-wrap gap-2">
				{#each services as svc (svc.name)}
					<li class="flex items-center gap-1.5 rounded-sm bg-muted px-2 py-1 text-xs">
						{#if svc.builds}
							<HammerIcon class="size-3.5" aria-label="built from source" />
						{:else}
							<BoxIcon class="size-3.5" aria-hidden="true" />
						{/if}
						<span class="font-mono">{svc.name}</span>
						{#if svc.ports.length}
							<span class="text-muted-foreground">:{svc.ports.join(', :')}</span>
						{/if}
					</li>
				{/each}
			</ul>
		</div>
	{/if}

	{#if pickWebService && services.length > 0 && errors.length === 0}
		<div class="space-y-1.5 pt-1">
			<Label for="{id}-web">Web service</Label>
			<Select.Root type="single" bind:value={webService} {disabled}>
				<Select.Trigger id="{id}-web" class="w-full sm:w-72">
					{webService ||
						(suggested ? `${suggested.name} (suggested)` : 'None — internal services only')}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">
						{suggested ? `${suggested.name} (suggested)` : 'Let Tond pick'}
					</Select.Item>
					{#each services as svc (svc.name)}
						<Select.Item value={svc.name}>
							{svc.name}{svc.ports.length ? ` — port ${svc.ports.join(', ')}` : ''}
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<p class="text-xs text-muted-foreground">
				This service gets the app's port and domain. You can give other services their own domains
				after the app is created.
			</p>
		</div>
	{/if}
</div>
