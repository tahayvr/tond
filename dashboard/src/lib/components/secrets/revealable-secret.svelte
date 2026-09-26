<script lang="ts">
	// A stored secret shown masked until someone explicitly reveals it.
	// Revealing calls the server (admin only; every reveal is audit-logged).
	// `value` can be set by the parent to show a secret it already has, such as
	// one returned once by a rotation.
	import { Button } from '$lib/components/ui/button/index.js';
	import { cn } from '$lib/utils';
	import Check from '~icons/ri/check-line';
	import Copy from '~icons/ri/file-copy-line';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';

	interface Props {
		id: string;
		/** Fetch the plaintext from the server */
		reveal: () => Promise<string>;
		/** Whether the current user may reveal (admins) */
		canReveal: boolean;
		/** A plaintext value the parent already holds (e.g. just rotated) */
		value?: string | null;
		/** Shown instead of the reveal button for users who can't reveal */
		lockedHint?: string;
		class?: string;
	}

	let {
		id,
		reveal,
		canReveal,
		value = $bindable(null),
		lockedHint = 'Only admins can reveal this.',
		class: className
	}: Props = $props();

	let revealing = $state(false);
	let error = $state('');
	let copied = $state(false);

	async function handleReveal() {
		error = '';
		revealing = true;
		try {
			value = await reveal();
		} catch (e: any) {
			error = e.message ?? 'Could not reveal the secret';
		} finally {
			revealing = false;
		}
	}

	async function handleCopy() {
		if (!value) return;
		try {
			await navigator.clipboard.writeText(value);
			copied = true;
			setTimeout(() => (copied = false), 2000);
		} catch {
			copied = false;
		}
	}
</script>

<div class={cn('grid gap-1.5', className)}>
	<div class="flex items-center gap-2">
		<input
			{id}
			readonly
			value={value ?? '••••••••••••••••'}
			aria-label={value ? 'Secret value' : 'Secret (hidden)'}
			class={cn(
				'flex h-9 w-full min-w-0 rounded-sm border border-input bg-background px-3 py-1 font-mono text-sm shadow-xs outline-none dark:bg-input/30',
				!value && 'text-muted-foreground'
			)}
		/>
		{#if value}
			<Button
				variant="outline"
				size="sm"
				class="h-9 shrink-0"
				onclick={handleCopy}
				aria-label="Copy secret"
			>
				{#if copied}
					<Check class="size-4" />
				{:else}
					<Copy class="size-4" />
				{/if}
			</Button>
			<Button
				variant="outline"
				size="sm"
				class="h-9 shrink-0"
				onclick={() => (value = null)}
				aria-label="Hide secret"
			>
				<EyeOff class="size-4" />
			</Button>
		{:else if canReveal}
			<Button
				variant="outline"
				size="sm"
				class="h-9 shrink-0"
				disabled={revealing}
				onclick={handleReveal}
			>
				<Eye class="size-4" />
				{revealing ? 'Revealing…' : 'Reveal'}
			</Button>
		{/if}
	</div>
	{#if error}
		<p class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">{error}</p>
	{:else if !value && !canReveal}
		<p class="text-xs text-muted-foreground">{lockedHint}</p>
	{:else if !value}
		<p class="text-xs text-muted-foreground">Hidden. Revealing it is recorded in Activity.</p>
	{/if}
</div>
