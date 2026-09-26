<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { maskUrlPassword } from '$lib/utils';
	import EyeIcon from '~icons/ri/eye-line';
	import EyeOffIcon from '~icons/ri/eye-off-line';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';

	let { value }: { value: string } = $props();

	let revealed = $state(false);
	let copied = $state(false);
	let masked = $derived(maskUrlPassword(value));
	let hasSecret = $derived(masked !== value);

	async function handleCopy() {
		await navigator.clipboard.writeText(value);
		copied = true;
		setTimeout(() => (copied = false), 1500);
	}
</script>

<div class="flex items-center gap-1 rounded bg-muted pr-1">
	<code
		class="block min-w-0 flex-1 truncate px-3 py-2 font-mono text-xs"
		title={revealed ? value : masked}
	>
		{revealed ? value : masked}
	</code>
	{#if hasSecret}
		<Button
			variant="ghost"
			size="icon"
			class="size-7 shrink-0"
			aria-label={revealed ? 'Hide password' : 'Show password'}
			onclick={() => (revealed = !revealed)}
		>
			{#if revealed}<EyeOffIcon class="size-4" />{:else}<EyeIcon class="size-4" />{/if}
		</Button>
	{/if}
	<Button
		variant="ghost"
		size="icon"
		class="size-7 shrink-0"
		aria-label="Copy connection string"
		onclick={handleCopy}
	>
		{#if copied}<CheckIcon class="size-4" />{:else}<CopyIcon class="size-4" />{/if}
	</Button>
</div>
