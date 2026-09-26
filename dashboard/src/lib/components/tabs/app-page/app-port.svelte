<script lang="ts">
	// The port the app listens on inside its container. Tond passes it as
	// $PORT, routes traffic to it and waits for it to answer before a cutover.
	import { updateContainerPort, type App } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { untrack } from 'svelte';

	interface Props {
		app: App;
		onSaved: (app: App) => void;
		/** Start a deploy after saving (the port applies on the next deploy). */
		onRedeploy: () => Promise<void>;
	}

	let { app, onSaved, onRedeploy }: Props = $props();

	let port = $state<number | null>(untrack(() => app.container_port));
	let saving = $state(false);
	let error = $state('');
	let saved = $state(false);

	$effect(() => {
		const p = app.container_port;
		untrack(() => (port = p));
	});

	let valid = $derived(port !== null && Number.isInteger(port) && port >= 1 && port <= 65535);
	let changed = $derived(port !== app.container_port);

	async function save(redeploy: boolean) {
		if (!valid || port === null) {
			error = 'Enter a port between 1 and 65535.';
			return;
		}
		saving = true;
		error = '';
		saved = false;
		try {
			const updated = await updateContainerPort(app.id, port);
			onSaved(updated);
			if (redeploy) await onRedeploy();
			else saved = true;
		} catch (e: any) {
			error = e.message ?? 'Could not save the port';
		} finally {
			saving = false;
		}
	}
</script>

<section class="grid gap-3" aria-labelledby="port-heading">
	<div>
		<h3 id="port-heading" class="text-sm font-medium">Port</h3>
		<p class="mt-1 text-sm text-muted-foreground">
			The port your app listens on inside its container. Tond sets <code>$PORT</code> to it and sends
			traffic there. nginx and most static servers use 80; many Node apps use 3000.
		</p>
	</div>
	<div class="flex flex-wrap items-end gap-3">
		<div class="grid gap-2">
			<Label for="container-port">Container port</Label>
			<Input
				id="container-port"
				type="number"
				bind:value={port}
				min={1}
				max={65535}
				class="w-32"
				disabled={saving}
			/>
		</div>
		<Button variant="outline" onclick={() => save(false)} disabled={saving || !changed}>
			{saving ? 'Saving…' : 'Save'}
		</Button>
		<Button onclick={() => save(true)} disabled={saving || !changed}>Save and redeploy</Button>
	</div>
	{#if error}
		<p class="rounded bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert">{error}</p>
	{:else if saved}
		<p class="text-xs text-muted-foreground">Saved. The new port applies on the next deploy.</p>
	{/if}
</section>
