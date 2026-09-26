<script lang="ts">
	// Publish one container port of a compose service and route domains to it.
	import { updateService, type ComposeService } from '$lib/api';
	import { isPlausibleDomain, normalizeDomain, parsePort, portChoices } from '$lib/compose';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import CloseIcon from '~icons/ri/close-line';
	import AddIcon from '~icons/ri/add-line';

	interface Props {
		open: boolean;
		appId: string;
		service: ComposeService | null;
		onSaved: (service: ComposeService | null) => void;
	}

	let { open = $bindable(false), appId, service, onSaved }: Props = $props();

	/** "" = internal only; a port number as text; "custom" = typed below. */
	let portChoice = $state('');
	let customPort = $state('');
	let domains = $state<string[]>([]);
	let newDomain = $state('');
	let error = $state('');
	let notice = $state('');
	let saving = $state(false);

	let choices = $derived(service ? portChoices(service) : []);

	$effect(() => {
		if (!open || !service) return;
		const p = service.published_port;
		portChoice = p == null ? '' : String(p);
		customPort = '';
		domains = [...service.domains];
		newDomain = '';
		error = '';
		notice = '';
		saving = false;
	});

	let publishedPort = $derived(
		portChoice === '' ? null : portChoice === 'custom' ? parsePort(customPort) : Number(portChoice)
	);

	function addDomain() {
		const d = normalizeDomain(newDomain);
		if (!d) return;
		if (!isPlausibleDomain(d)) {
			error = `${d} doesn't look like a domain. Use a full name such as shop.example.com.`;
			return;
		}
		if (!domains.includes(d)) domains = [...domains, d];
		newDomain = '';
		error = '';
	}

	async function handleSave() {
		if (!service) return;
		if (newDomain.trim()) addDomain();
		if (error) return;
		if (portChoice === 'custom' && publishedPort === null) {
			error = 'Enter a container port between 1 and 65535.';
			return;
		}
		if (domains.length && publishedPort === null) {
			error = 'Publish a container port first: domains are routed to it.';
			return;
		}
		saving = true;
		error = '';
		try {
			const result = await updateService(appId, service.name, {
				published_port: publishedPort,
				domains
			});
			onSaved(result.service);
			if (result.redeploy_required) {
				notice =
					'Saved. The new port is bound on the next deploy; domains route there once it has run.';
			} else {
				open = false;
			}
		} catch (e: any) {
			error = e.message ?? 'Could not save the service routing';
		} finally {
			saving = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title
				>Domains &amp; port · <span class="font-mono normal-case">{service?.name}</span
				></Dialog.Title
			>
			<Dialog.Description>
				Tond publishes one container port of this service on 127.0.0.1 and routes these domains to
				it over HTTPS. Services without a port stay reachable only from the app's other services.
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-5 py-2">
			<fieldset class="space-y-2">
				<legend class="text-sm font-medium">Container port</legend>
				<div class="flex flex-wrap gap-2" role="radiogroup" aria-label="Container port">
					<Button
						type="button"
						size="sm"
						variant={portChoice === '' ? 'default' : 'outline'}
						role="radio"
						aria-checked={portChoice === ''}
						onclick={() => (portChoice = '')}>Internal only</Button
					>
					{#each choices as port (port)}
						<Button
							type="button"
							size="sm"
							variant={portChoice === String(port) ? 'default' : 'outline'}
							class="font-mono"
							role="radio"
							aria-checked={portChoice === String(port)}
							onclick={() => (portChoice = String(port))}>{port}</Button
						>
					{/each}
					<Button
						type="button"
						size="sm"
						variant={portChoice === 'custom' ? 'default' : 'outline'}
						role="radio"
						aria-checked={portChoice === 'custom'}
						onclick={() => (portChoice = 'custom')}>Other…</Button
					>
				</div>
				{#if portChoice === 'custom'}
					<div class="space-y-1">
						<Label for="svc-custom-port" class="text-xs">Port the service listens on</Label>
						<Input
							id="svc-custom-port"
							inputmode="numeric"
							class="w-32 font-mono"
							placeholder="8080"
							bind:value={customPort}
							disabled={saving}
						/>
					</div>
				{/if}
				{#if choices.length === 0}
					<p class="text-xs text-muted-foreground">
						The compose file doesn't declare a port for this service (ports: or expose:). Pick
						"Other…" and enter the port it listens on.
					</p>
				{/if}
			</fieldset>

			<div class="space-y-2">
				<Label for="svc-domain">Domains</Label>
				{#if domains.length}
					<ul class="space-y-1.5">
						{#each domains as domain (domain)}
							<li class="flex items-center justify-between gap-2 rounded-sm border px-2 py-1">
								<span class="truncate font-mono text-sm">{domain}</span>
								<Button
									type="button"
									variant="ghost"
									size="sm"
									class="h-7 px-2"
									aria-label="Remove {domain}"
									onclick={() => (domains = domains.filter((d) => d !== domain))}
									disabled={saving}
								>
									<CloseIcon class="size-4" />
								</Button>
							</li>
						{/each}
					</ul>
				{/if}
				<div class="flex gap-2">
					<Input
						id="svc-domain"
						placeholder="shop.example.com"
						class="font-mono"
						autocomplete="off"
						spellcheck={false}
						bind:value={newDomain}
						disabled={saving || publishedPort === null}
						onkeydown={(e) => {
							if (e.key === 'Enter') {
								e.preventDefault();
								addDomain();
							}
						}}
					/>
					<Button
						type="button"
						variant="outline"
						onclick={addDomain}
						disabled={saving || !newDomain.trim() || publishedPort === null}
					>
						<AddIcon class="size-4" /> Add
					</Button>
				</div>
				<p class="text-xs text-muted-foreground">
					Point each domain's DNS at this server. Tond gets a certificate for it automatically.
				</p>
			</div>

			<InlineError message={error} ondismiss={() => (error = '')} />
			{#if notice}
				<p class="rounded-sm border border-info/40 bg-info/10 px-3 py-2 text-sm" role="status">
					{notice}
				</p>
			{/if}
		</div>

		<Dialog.Footer>
			<Button variant="outline" onclick={() => (open = false)} disabled={saving}
				>{notice ? 'Close' : 'Cancel'}</Button
			>
			<Button onclick={handleSave} disabled={saving} aria-busy={saving}>
				{saving ? 'Saving…' : 'Save'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
