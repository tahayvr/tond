<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';

	interface Props {
		domain: string;
		healthCheckPath: string;
		healthCheckInterval: number;
		healthFailureThreshold: number;
		drainTimeout: number;
	}

	let {
		domain = $bindable(''),
		healthCheckPath = $bindable('/health'),
		healthCheckInterval = $bindable(30),
		healthFailureThreshold = $bindable(3),
		drainTimeout = $bindable(30)
	}: Props = $props();
</script>

<div class="space-y-6">
	<div class="space-y-2">
		<Label for="domain">Custom Domain (Optional)</Label>
		<Input id="domain" type="text" bind:value={domain} placeholder="app.example.com" />
		<p class="text-xs text-muted-foreground">
			Leave empty to use the generated subdomain. DNS must point to this server.
		</p>
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
