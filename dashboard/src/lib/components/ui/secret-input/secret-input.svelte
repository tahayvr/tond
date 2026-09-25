<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '$lib/utils.js';
	import Eye from '~icons/ri/eye-line';
	import EyeOff from '~icons/ri/eye-off-line';
	import Copy from '~icons/ri/file-copy-line';
	import Check from '~icons/ri/check-line';

	type Props = WithElementRef<Omit<HTMLInputAttributes, 'type'>> & {
		/** Show the copy button (default: true) */
		copyable?: boolean;
		/** Show the reveal button (default: true) */
		revealable?: boolean;
	};

	let {
		ref = $bindable(null),
		value = $bindable(),
		class: className,
		copyable = true,
		revealable = true,
		'data-slot': dataSlot = 'input',
		...restProps
	}: Props = $props();

	let revealed = $state(false);
	let copied = $state(false);
	let copyTimeout: ReturnType<typeof setTimeout> | null = null;

	async function handleCopy() {
		if (!value) return;
		await navigator.clipboard.writeText(String(value));
		copied = true;
		if (copyTimeout) clearTimeout(copyTimeout);
		copyTimeout = setTimeout(() => {
			copied = false;
		}, 2000);
	}

	// How many action buttons are rendered — used to compute right padding
	let buttonCount = $derived((copyable ? 1 : 0) + (revealable ? 1 : 0));
	// Each button is 28px wide + 4px gap, plus 4px outer padding
	let paddingRight = $derived(`${buttonCount * 32 + 4}px`);
</script>

<div class="relative flex items-center">
	<input
		bind:this={ref}
		data-slot={dataSlot}
		class={cn(
			'flex h-9 w-full min-w-0 rounded-sm border border-input bg-background px-3 py-1 font-mono text-sm shadow-xs ring-offset-background transition-[color,box-shadow] outline-none selection:bg-primary selection:text-primary-foreground placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50 dark:bg-input/30',
			'focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50',
			'aria-invalid:border-destructive aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40',
			className
		)}
		style="padding-right: {paddingRight}"
		type={revealed ? 'text' : 'password'}
		bind:value
		{...restProps}
	/>

	<div class="absolute right-1 flex items-center gap-1">
		{#if copyable}
			<button
				type="button"
				onclick={handleCopy}
				class="flex h-7 w-7 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				aria-label="Copy to clipboard"
			>
				{#if copied}
					<Check class="h-3.5 w-3.5 text-green-500" />
				{:else}
					<Copy class="h-3.5 w-3.5" />
				{/if}
			</button>
		{/if}

		{#if revealable}
			<button
				type="button"
				onclick={() => (revealed = !revealed)}
				class="flex h-7 w-7 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				aria-label={revealed ? 'Hide value' : 'Reveal value'}
			>
				{#if revealed}
					<EyeOff class="h-3.5 w-3.5" />
				{:else}
					<Eye class="h-3.5 w-3.5" />
				{/if}
			</button>
		{/if}
	</div>
</div>
