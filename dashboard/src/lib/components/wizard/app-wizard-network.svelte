<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';

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
	}

	let {
		domain = $bindable(''),
		appName,
		appsDomain = null,
		dashboardDomain = '',
		healthCheckPath = $bindable('/health'),
		healthCheckInterval = $bindable(30),
		healthFailureThreshold = $bindable(3),
		drainTimeout = $bindable(30)
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
</script>

<div class="space-y-6">
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
					HTTP path for health checks. Set empty to disable.
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
