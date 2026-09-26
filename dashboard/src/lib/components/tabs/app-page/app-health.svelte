<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Separator } from '$lib/components/ui/separator/index.js';
	import type { App } from '$lib/api';

	interface Props {
		app: App;
		healthPath: string;
		healthInterval: number;
		healthThreshold: number;
		drainTimeout: number;
		onUpdateHealth: () => void;
		onDisableHealth: () => void;
		onUpdateDrainTimeout: () => void;
		/** Show the health check settings (web apps only). */
		showHealthCheck?: boolean;
	}

	let {
		app,
		healthPath = $bindable(''),
		healthInterval = $bindable(30),
		healthThreshold = $bindable(3),
		drainTimeout = $bindable(30),
		onUpdateHealth,
		onDisableHealth,
		onUpdateDrainTimeout,
		showHealthCheck = true
	}: Props = $props();
</script>

<div class="space-y-4 pt-4">
	{#if showHealthCheck}
		<p class="text-sm text-muted-foreground">
			Configure an HTTP health check endpoint. The server will poll this path periodically and
			auto-restart the container after consecutive failures.
		</p>
		<div class="grid gap-4">
			<div class="grid gap-2">
				<Label for="health-path">Health check path</Label>
				<Input id="health-path" type="text" bind:value={healthPath} placeholder="/health" />
				<p class="text-xs text-muted-foreground">Leave empty to disable health checks.</p>
			</div>
			<div class="grid grid-cols-2 gap-4">
				<div class="grid gap-2">
					<Label for="health-interval">Interval (seconds)</Label>
					<Input id="health-interval" type="number" bind:value={healthInterval} min={5} max={300} />
				</div>
				<div class="grid gap-2">
					<Label for="health-threshold">Failure threshold</Label>
					<Input
						id="health-threshold"
						type="number"
						bind:value={healthThreshold}
						min={1}
						max={10}
					/>
				</div>
			</div>
			{#if app.health_check_path}
				<div class="rounded-md border p-3 text-sm">
					<div class="font-medium">Current status</div>
					<div class="mt-1 text-muted-foreground">
						Polling <span class="font-mono">{app.health_check_path}</span> every {app.health_check_interval}s
					</div>
					{#if app.health_consecutive_failures > 0}
						<div class="mt-1 text-destructive">
							{app.health_consecutive_failures} consecutive failure(s) (threshold: {app.health_failure_threshold})
						</div>
					{:else}
						<div class="mt-1 text-success">No recent failures</div>
					{/if}
				</div>
			{/if}
		</div>
		<div class="flex gap-3">
			<Button onclick={onUpdateHealth}>Save Health Check</Button>
			{#if app.health_check_path}
				<Button variant="outline" onclick={onDisableHealth}>Disable</Button>
			{/if}
		</div>

		<Separator />
	{/if}

	<div class="grid gap-3">
		<div>
			<h3 class="text-sm font-medium">Graceful Deploy Timeout</h3>
			<p class="mt-1 text-sm text-muted-foreground">
				Deploys wait up to this many seconds for the new container to pass the health check (or,
				without one, to answer on its port; at least 60 s) before giving up and keeping the previous
				version. It is also how long the old container gets to finish requests.
			</p>
		</div>
		<div class="flex items-end gap-3">
			<div class="grid gap-2">
				<Label for="drain-timeout">Timeout (seconds)</Label>
				<Input
					id="drain-timeout"
					type="number"
					bind:value={drainTimeout}
					min={5}
					max={300}
					class="w-32"
				/>
			</div>
			<Button onclick={onUpdateDrainTimeout}>Save</Button>
		</div>
	</div>
</div>
