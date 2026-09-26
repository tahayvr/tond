<script lang="ts">
	import { updateSettings, type ServerSettings } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import TimerIcon from '~icons/ri/timer-line';

	interface Props {
		settings: ServerSettings;
		canEdit: boolean;
	}

	let { settings, canEdit }: Props = $props();

	let idleHours = $state(12);
	let maxDays = $state(30);
	let saving = $state(false);
	let saved = $state(false);
	let error = $state('');

	$effect(() => {
		idleHours = settings.session_idle_timeout_hours ?? 12;
		maxDays = settings.session_max_age_days ?? 30;
	});

	async function handleSubmit(e: Event) {
		e.preventDefault();
		error = '';
		saved = false;
		if (!Number.isInteger(idleHours) || idleHours < 1 || idleHours > 720) {
			error = 'The idle timeout must be a whole number of hours between 1 and 720.';
			return;
		}
		if (!Number.isInteger(maxDays) || maxDays < 1 || maxDays > 365) {
			error = 'The session lifetime must be a whole number of days between 1 and 365.';
			return;
		}
		saving = true;
		try {
			const updated = await updateSettings({
				session_idle_timeout_hours: idleHours,
				session_max_age_days: maxDays
			});
			idleHours = updated.session_idle_timeout_hours ?? idleHours;
			maxDays = updated.session_max_age_days ?? maxDays;
			saved = true;
			setTimeout(() => (saved = false), 3000);
		} catch (err: any) {
			error = err.message;
		} finally {
			saving = false;
		}
	}
</script>

<form class="rounded-lg border border-border bg-card p-4" onsubmit={handleSubmit} novalidate>
	<div class="mb-1 flex items-center gap-2">
		<TimerIcon class="size-4 text-muted-foreground/70" />
		<h3 class="font-semibold">Dashboard sessions</h3>
	</div>
	<p class="mb-4 max-w-2xl text-sm text-muted-foreground">
		Signed-in browsers are signed out after a period without activity, and in any case after the
		maximum lifetime. Shorter is safer on shared machines.
	</p>
	<div class="grid max-w-xl gap-4 sm:grid-cols-2">
		<div class="grid gap-1.5">
			<Label for="idle-hours">Sign out after inactivity (hours)</Label>
			<Input
				id="idle-hours"
				type="number"
				min={1}
				max={720}
				bind:value={idleHours}
				disabled={!canEdit}
			/>
		</div>
		<div class="grid gap-1.5">
			<Label for="max-days">Maximum session lifetime (days)</Label>
			<Input
				id="max-days"
				type="number"
				min={1}
				max={365}
				bind:value={maxDays}
				disabled={!canEdit}
			/>
		</div>
	</div>
	{#if error}
		<p class="mt-3 rounded-md bg-destructive/10 p-3 text-sm text-destructive" role="alert">
			{error}
		</p>
	{/if}
	<div class="mt-4 flex items-center gap-3">
		{#if canEdit}
			<Button type="submit" size="sm" disabled={saving}>{saving ? 'Saving…' : 'Save'}</Button>
			{#if saved}<span class="text-xs text-success" role="status"
					>Saved. Applies to every session now.</span
				>{/if}
		{:else}
			<p class="text-xs text-muted-foreground">Only admins can change this.</p>
		{/if}
	</div>
</form>
