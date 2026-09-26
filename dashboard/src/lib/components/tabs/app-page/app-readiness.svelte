<script lang="ts">
	// What kind of app this is, which decides how a deploy knows the new
	// container is ready: a web app (HTTP), a TCP service, or a background
	// worker (no port, no domain; ready once it stays up for N seconds).
	import { untrack } from 'svelte';
	import { appReadiness, updateReadiness, type App, type Readiness } from '$lib/api';
	import { cn } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { confirmAction } from '$lib/confirm.svelte';

	interface Props {
		app: App;
		onSaved: (app: App) => void;
		/** The user may change app settings (apps:write). */
		canEdit?: boolean;
	}

	let { app, onSaved, canEdit = true }: Props = $props();

	const MODES: { value: Readiness; label: string; desc: string }[] = [
		{
			value: 'http',
			label: 'Web app',
			desc: 'Ready when the health check passes, or when anything answers HTTP on the port.'
		},
		{
			value: 'tcp',
			label: 'TCP service',
			desc: 'Ready when the port accepts connections.'
		},
		{
			value: 'none',
			label: 'Background worker',
			desc: 'No port, no domain. Ready once it stays up without exiting.'
		}
	];

	let mode = $state<Readiness>(untrack(() => appReadiness(app)));
	let secs = $state<number>(untrack(() => app.ready_after_secs ?? 10));
	let saving = $state(false);
	let error = $state('');
	let saved = $state(false);

	$effect(() => {
		const current = appReadiness(app);
		const currentSecs = app.ready_after_secs ?? 10;
		untrack(() => {
			mode = current;
			secs = currentSecs;
		});
	});

	let changed = $derived(
		mode !== appReadiness(app) || (mode === 'none' && secs !== (app.ready_after_secs ?? 10))
	);
	/** Becoming a worker drops the domain: ask first. */
	let losesDomain = $derived(mode === 'none' && appReadiness(app) !== 'none' && !!app.domain);

	async function handleSave() {
		if (saving) return;
		if (mode === 'none' && (!Number.isInteger(secs) || secs < 1 || secs > 600)) {
			error = 'Enter how long the worker must stay up (1-600 seconds).';
			return;
		}
		if (losesDomain) {
			const ok = await confirmAction({
				title: `Make ${app.name} a background worker?`,
				description: `Workers serve no traffic. From the next deploy, it counts as ready once it stays up for ${secs}s.`,
				details: [
					`The domain ${app.domain} is removed and the app is no longer routed.`,
					'The health check path is removed.'
				],
				confirmLabel: 'Make it a worker'
			});
			if (!ok) return;
		}
		await save();
	}

	async function save() {
		saving = true;
		error = '';
		saved = false;
		try {
			const updated = await updateReadiness(app.id, mode, mode === 'none' ? secs : undefined);
			onSaved(updated);
			saved = true;
		} catch (e: any) {
			error = e.message ?? 'Could not save';
		} finally {
			saving = false;
		}
	}
</script>

<section class="grid gap-3" aria-labelledby="readiness-heading">
	<div>
		<h3 id="readiness-heading" class="text-sm font-medium">Kind of app</h3>
		<p class="mt-1 text-sm text-muted-foreground">
			Decides how a deploy knows the new container is ready before the old one is stopped.
		</p>
	</div>
	<div class="grid gap-2 sm:grid-cols-3" role="radiogroup" aria-label="Kind of app">
		{#each MODES as m (m.value)}
			<button
				type="button"
				role="radio"
				aria-checked={mode === m.value}
				disabled={!canEdit || saving || (m.value === 'none' && app.source_type === 'compose')}
				class={cn(
					'rounded-md border p-3 text-left transition-colors hover:bg-accent/50 disabled:cursor-not-allowed disabled:opacity-60',
					mode === m.value && 'border-foreground bg-accent/40'
				)}
				onclick={() => (mode = m.value)}
			>
				<span class="block text-sm font-medium">{m.label}</span>
				<span class="mt-1 block text-xs text-muted-foreground">{m.desc}</span>
			</button>
		{/each}
	</div>
	{#if mode === 'none'}
		<div class="grid gap-2">
			<Label for="ready-after-secs">Stays up for (seconds)</Label>
			<Input
				id="ready-after-secs"
				type="number"
				bind:value={secs}
				min={1}
				max={600}
				class="w-32"
				disabled={!canEdit || saving}
			/>
		</div>
	{/if}
	<div class="flex items-center gap-3">
		<Button variant="outline" onclick={handleSave} disabled={!canEdit || saving || !changed}>
			{saving ? 'Saving…' : 'Save'}
		</Button>
		{#if saved && !changed}
			<span class="text-xs text-muted-foreground">Saved. Applies on the next deploy.</span>
		{/if}
	</div>
	{#if !canEdit}
		<p class="text-xs text-muted-foreground">Your role can't change app settings.</p>
	{/if}
	<InlineError message={error} ondismiss={() => (error = '')} />
</section>
