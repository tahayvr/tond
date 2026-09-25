<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import KeyRound from '~icons/ri/key-2-line';

	interface Props {
		currentPassword: string;
		newPassword: string;
		confirmPassword: string;
		saving: boolean;
		saved: boolean;
		error: string;
		onSubmit: () => void;
	}

	let {
		currentPassword = $bindable(''),
		newPassword = $bindable(''),
		confirmPassword = $bindable(''),
		saving,
		saved,
		error,
		onSubmit
	}: Props = $props();
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-4 flex items-center gap-2">
		<KeyRound class="h-4 w-4 text-muted-foreground/70" />
		<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
			CHANGE_PASSWORD
		</span>
	</div>

	<form
		class="max-w-sm space-y-4"
		onsubmit={(e) => {
			e.preventDefault();
			onSubmit();
		}}
	>
		<div class="grid gap-2">
			<Label for="current-pw" class="font-mono text-[10px] text-muted-foreground uppercase">
				Current Password
			</Label>
			<Input
				id="current-pw"
				type="password"
				bind:value={currentPassword}
				autocomplete="current-password"
				class="border-border bg-card font-mono text-sm"
				required
			/>
		</div>

		<div class="h-px bg-border"></div>

		<div class="grid gap-2">
			<Label for="new-pw" class="font-mono text-[10px] text-muted-foreground uppercase">
				New Password
			</Label>
			<Input
				id="new-pw"
				type="password"
				bind:value={newPassword}
				autocomplete="new-password"
				class="border-border bg-card font-mono text-sm"
				required
			/>
		</div>

		<div class="grid gap-2">
			<Label for="confirm-pw" class="font-mono text-[10px] text-muted-foreground uppercase">
				Confirm New Password
			</Label>
			<Input
				id="confirm-pw"
				type="password"
				bind:value={confirmPassword}
				autocomplete="new-password"
				class="border-border bg-card font-mono text-sm"
				required
			/>
		</div>

		{#if error}
			<div
				class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive"
			>
				{error}
			</div>
		{/if}

		<div class="flex items-center gap-3">
			<Button type="submit" disabled={saving}>
				{saving ? 'Saving…' : 'Change Password'}
			</Button>
			{#if saved}
				<span class="font-mono text-[10px] text-green-500">SAVED</span>
			{/if}
		</div>
	</form>
</div>
