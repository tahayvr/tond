<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import Server from '@lucide/svelte/icons/server';
	import Settings from '@lucide/svelte/icons/settings';

	interface Props {
		serverName: string;
		dashboardDomain: string;
		isSaving?: boolean;
		saved?: boolean;
		error: string;
		onSubmit: () => void;
	}

	let {
		serverName = $bindable(''),
		dashboardDomain = $bindable(''),
		isSaving = false,
		saved = false,
		error,
		onSubmit
	}: Props = $props();
</script>

<form
	class="space-y-4"
	onsubmit={(e) => {
		e.preventDefault();
		onSubmit();
	}}
>
	<!-- Server Identity -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<Server class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				SERVER_IDENTITY
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="server-name" class="font-mono text-[10px] text-muted-foreground uppercase">
				Server Name
			</Label>
			<Input
				id="server-name"
				type="text"
				bind:value={serverName}
				placeholder="My Tond Server"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				A human-readable name for this server instance.
			</p>
		</div>
	</div>

	<!-- Access Configuration -->
	<div class="rounded-lg border border-border bg-card p-4">
		<div class="mb-4 flex items-center gap-2">
			<Settings class="h-4 w-4 text-muted-foreground/70" />
			<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
				ACCESS_CONFIGURATION
			</span>
		</div>

		<div class="grid gap-2">
			<Label for="dashboard-domain" class="font-mono text-[10px] text-muted-foreground uppercase">
				Dashboard Domain
			</Label>
			<Input
				id="dashboard-domain"
				type="text"
				bind:value={dashboardDomain}
				placeholder="tond.example.com"
				class="border-border bg-card font-mono text-sm"
			/>
			<p class="font-mono text-[10px] text-muted-foreground/60">
				Leave blank for direct IP access. If you set a domain, point its A record at this server.
			</p>
		</div>
	</div>

	{#if error}
		<div class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-destructive">
			{error}
		</div>
	{/if}

	<div class="flex items-center gap-3">
		<Button type="submit" disabled={isSaving}>
			{isSaving ? 'Saving…' : 'Save'}
		</Button>
		{#if saved}
			<span class="font-mono text-[10px] text-success">SAVED</span>
		{/if}
	</div>
</form>
