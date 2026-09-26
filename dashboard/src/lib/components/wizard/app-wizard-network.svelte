<script lang="ts">
	// Network step: the container port (asked for every source type, with a
	// hint from the Dockerfile, compose file or image when Tond can tell),
	// domain and health checks.
	import { untrack } from 'svelte';
	import { inspectImage } from '$lib/api';
	import { parseComposePorts, parseDockerfileExpose } from '$lib/wizard-detect';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import type { SourceSubType } from '$lib/components/wizard/app-wizard-source.svelte';
	import ScanIcon from '~icons/ri/search-line';
	import InfoIcon from '~icons/ri/information-line';

	interface Props {
		domain: string;
		appName: string;
		/** Settings' apps domain; null while loading, '' when none is set */
		appsDomain?: string | null;
		dashboardDomain?: string;
		healthCheckPath: string;
		healthCheckInterval: number;
		healthFailureThreshold: number;
		drainTimeout: number;
		containerPort: number;
		sourceSubType: SourceSubType;
		sourceUrl: string;
		dockerfileContent: string;
		composeContent: string;
		registryCredentialId: string;
		buildMethod: 'dockerfile' | 'tbuild';
		/** A template's default port, when a template is selected. */
		templatePort: number | null;
		/** The user typed a port; don't overwrite it with a detected one. */
		portTouched: boolean;
	}

	let {
		domain = $bindable(''),
		appName,
		appsDomain = null,
		dashboardDomain = '',
		healthCheckPath = $bindable('/health'),
		healthCheckInterval = $bindable(30),
		healthFailureThreshold = $bindable(3),
		drainTimeout = $bindable(30),
		containerPort = $bindable(8080),
		sourceSubType,
		sourceUrl,
		dockerfileContent,
		composeContent,
		registryCredentialId,
		buildMethod,
		templatePort,
		portTouched = $bindable(false)
	}: Props = $props();

	const normalize = (d: string) => d.trim().toLowerCase().replace(/\.$/, '');
	const site = (d: string) => d.split('.').slice(-2).join('.');

	let automaticHost = $derived(
		appsDomain ? `${normalize(appName) || '<app-name>'}.${appsDomain}` : ''
	);
	// Early hint only — tond-server does the Public Suffix List check and
	// returns the same warning on the app page.
	let sameSiteAsDashboard = $derived.by(() => {
		const d = normalize(domain);
		const dash = normalize(dashboardDomain);
		if (!d || !dash) return false;
		return d === dash || d.endsWith(`.${dash}`) || dash.endsWith(`.${d}`) || site(d) === site(dash);
	});
	let detecting = $state(false);
	let detectError = $state('');
	let imagePorts = $state<number[] | null>(null);
	let imageEnvPort = $state<number | null>(null);
	let detectedFor = $state('');

	let filePorts = $derived(
		sourceSubType === 'dockerfile'
			? parseDockerfileExpose(dockerfileContent)
			: sourceSubType === 'compose'
				? parseComposePorts(composeContent)
				: []
	);

	/** The port Tond would suggest from what it knows, if anything. */
	let suggested = $derived.by((): number | null => {
		if (sourceSubType === 'template') return templatePort;
		if (sourceSubType === 'dockerfile' || sourceSubType === 'compose') return filePorts[0] ?? null;
		if (sourceSubType === 'image') {
			if (imageEnvPort && (!imagePorts?.length || imagePorts.includes(imageEnvPort)))
				return imageEnvPort;
			return imagePorts?.[0] ?? null;
		}
		return null;
	});

	// Apply a detected port unless the user chose one.
	$effect(() => {
		const s = suggested;
		if (s && untrack(() => !portTouched)) containerPort = s;
	});

	async function detectImage() {
		const image = sourceUrl.trim();
		if (!image) return;
		detecting = true;
		detectError = '';
		try {
			const info = await inspectImage(image, registryCredentialId || undefined);
			imagePorts = info.exposed_ports;
			imageEnvPort = info.env_port;
			detectedFor = image;
		} catch (e: any) {
			detectError = e.message ?? "Couldn't inspect the image";
			imagePorts = null;
		} finally {
			detecting = false;
		}
	}

	// Look at the image's ports once per image (the first deploy pulls it anyway).
	$effect(() => {
		if (sourceSubType !== 'image') return;
		const image = sourceUrl.trim();
		if (!image || untrack(() => detectedFor === image || detecting)) return;
		untrack(detectImage);
	});

	function handlePortInput() {
		portTouched = true;
	}

	function applyPort(p: number) {
		containerPort = p;
		portTouched = true;
	}
</script>

<div class="space-y-6">
	<section class="space-y-2">
		<Label for="container-port">Container port *</Label>
		<div class="flex flex-wrap items-center gap-2">
			<Input
				id="container-port"
				type="number"
				bind:value={containerPort}
				oninput={handlePortInput}
				min={1}
				max={65535}
				class="w-32"
			/>
			{#if sourceSubType === 'image'}
				<Button
					variant="outline"
					size="sm"
					onclick={detectImage}
					disabled={detecting || !sourceUrl.trim()}
				>
					<ScanIcon />
					{detecting ? 'Checking the image…' : 'Detect from image'}
				</Button>
			{/if}
		</div>
		<p class="text-xs text-muted-foreground">
			The port your app listens on inside the container. Tond passes it as <code>$PORT</code> and sends
			traffic there; the deploy waits until the app answers on it.
		</p>

		<div class="flex items-start gap-2 rounded-md bg-muted/40 px-3 py-2 text-xs">
			<InfoIcon class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
			<div class="space-y-1">
				{#if sourceSubType === 'image'}
					{#if detecting}
						<p>Pulling the image to read its ports…</p>
					{:else if detectError}
						<p class="text-destructive">{detectError}</p>
					{:else if imagePorts && imagePorts.length > 0}
						<p>
							The image exposes {imagePorts.join(', ')}.
							{#if imagePorts.length > 1}Pick the one that serves HTTP.{/if}
						</p>
					{:else if imagePorts}
						<p>
							The image doesn't declare a port{imageEnvPort
								? `, but sets PORT=${imageEnvPort}`
								: ''}. Check its documentation (nginx uses 80, many Node apps 3000).
						</p>
					{:else}
						<p>Tond reads the port from the image once you enter it on the Source step.</p>
					{/if}
				{:else if sourceSubType === 'dockerfile'}
					<p>
						{filePorts.length
							? `Your Dockerfile EXPOSEs ${filePorts.join(', ')}.`
							: "Your Dockerfile doesn't EXPOSE a port; use the one your server listens on."}
					</p>
				{:else if sourceSubType === 'compose'}
					<p>
						{filePorts.length
							? `Your compose file uses container port${filePorts.length > 1 ? 's' : ''} ${filePorts.join(', ')}. Tond routes to the service that publishes the port you pick.`
							: 'Publish the web service’s port in your compose file (ports: ["80"]) and enter it here.'}
					</p>
				{:else if sourceSubType === 'template'}
					<p>The template's default{templatePort ? ` is ${templatePort}` : ''}.</p>
				{:else if buildMethod === 'tbuild'}
					<p>
						Images TBuild generates listen on <code>$PORT</code>, so 8080 works unless your code
						hard-codes another port.
					</p>
				{:else}
					<p>Use the port your Dockerfile EXPOSEs or your server listens on.</p>
				{/if}
				{#if suggested && suggested !== containerPort}
					<Button
						variant="link"
						size="sm"
						class="h-auto p-0 text-xs"
						onclick={() => suggested && applyPort(suggested)}
					>
						Use {suggested}
					</Button>
				{:else if (sourceSubType === 'dockerfile' || sourceSubType === 'compose' || sourceSubType === 'image') && filePorts.length + (imagePorts?.length ?? 0) > 1}
					<div class="flex flex-wrap gap-1">
						{#each sourceSubType === 'image' ? (imagePorts ?? []) : filePorts as p (p)}
							<Button
								variant={p === containerPort ? 'secondary' : 'outline'}
								size="sm"
								class="h-6 px-2 text-xs"
								onclick={() => applyPort(p)}>{p}</Button
							>
						{/each}
					</div>
				{/if}
			</div>
		</div>
	</section>

	<Separator class="bg-border" />

	<div class="space-y-2">
		<Label for="domain">Custom Domain {appsDomain === '' ? '' : '(Optional)'}</Label>
		<Input id="domain" type="text" bind:value={domain} placeholder="app.example.com" />
		{#if appsDomain}
			<p class="text-xs text-muted-foreground">
				Leave empty and the app is served at
				<span class="font-mono text-foreground">{automaticHost}</span>. A custom domain needs an A
				record pointing at this server.
			</p>
		{:else if appsDomain === ''}
			<p class="rounded border border-warn/40 bg-warn/10 px-3 py-2 text-xs">
				<span class="font-medium text-warn">No apps domain is set,</span>
				<span class="text-muted-foreground">
					so without a custom domain this app has no public address. Enter one here (with an A
					record pointing at this server), or set an apps domain in
					<a href="/settings" class="underline underline-offset-2">Settings</a> to give every app
					<span class="font-mono">&lt;app&gt;.&lt;apps-domain&gt;</span> automatically.
				</span>
			</p>
		{:else}
			<p class="text-xs text-muted-foreground">
				Leave empty to use the automatic address. DNS must point to this server.
			</p>
		{/if}
		{#if sameSiteAsDashboard}
			<p class="text-xs text-warn">
				This is on the same site as your dashboard ({normalize(dashboardDomain)}). It works, but the
				app could send same-site requests to the dashboard and set its cookies — use a domain on a
				different site for code you don't fully trust.
			</p>
		{/if}
	</div>

	<Separator class="bg-border" />

	<div class="space-y-4">
		<h4 class="font-mono text-sm tracking-wider uppercase">Health Check Settings</h4>

		<div class="grid gap-4 sm:grid-cols-2">
			<div class="space-y-2">
				<Label for="health-path">Health Check Path</Label>
				<Input id="health-path" type="text" bind:value={healthCheckPath} placeholder="/health" />
				<p class="text-xs text-muted-foreground">
					HTTP path that answers 2xx when the app is ready. Leave empty to only wait for the port.
				</p>
			</div>

			<div class="space-y-2">
				<Label for="health-interval">Check Interval (seconds)</Label>
				<Input
					id="health-interval"
					type="number"
					bind:value={healthCheckInterval}
					min={5}
					max={300}
				/>
				<p class="text-xs text-muted-foreground">Range: 5-300 seconds</p>
			</div>
		</div>

		<div class="space-y-2">
			<Label for="health-threshold">Failure Threshold</Label>
			<Input
				id="health-threshold"
				type="number"
				bind:value={healthFailureThreshold}
				min={1}
				max={10}
			/>
			<p class="text-xs text-muted-foreground">
				Consecutive failures before marking unhealthy (1-10)
			</p>
		</div>
	</div>

	<Separator class="bg-border" />

	<div class="space-y-2">
		<Label for="drain-timeout">Graceful Shutdown Timeout (seconds)</Label>
		<Input id="drain-timeout" type="number" bind:value={drainTimeout} min={1} max={300} />
		<p class="text-xs text-muted-foreground">
			Time to wait for connections to close before forceful shutdown
		</p>
	</div>
</div>
