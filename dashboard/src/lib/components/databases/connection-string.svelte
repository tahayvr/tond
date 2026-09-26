<script lang="ts">
	// A connection string as the server returns it: password masked. The
	// plaintext (`plain`) is only known after an explicit, audited credentials
	// reveal; showing or copying a masked string asks the page to reveal first.
	import { Button } from '$lib/components/ui/button/index.js';
	import EyeIcon from '~icons/ri/eye-line';
	import EyeOffIcon from '~icons/ri/eye-off-line';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';

	interface Props {
		/** The string from the API (password masked) */
		value: string;
		/** The plaintext string, once revealed */
		plain?: string | null;
		/** Reveal the credentials (admins); resolves when `plain` is available */
		reveal?: () => Promise<void>;
		canReveal?: boolean;
	}

	let { value, plain = null, reveal, canReveal = false }: Props = $props();

	let shown = $state(false);
	let copied = $state(false);
	let busy = $state(false);
	let hasSecret = $derived(value.includes('••••••••'));
	let text = $derived(shown && plain ? plain : value);

	async function ensurePlain(): Promise<boolean> {
		if (plain || !hasSecret) return true;
		if (!reveal) return false;
		busy = true;
		try {
			await reveal();
			return true;
		} catch {
			return false;
		} finally {
			busy = false;
		}
	}

	async function handleToggle() {
		if (shown) {
			shown = false;
			return;
		}
		if (await ensurePlain()) shown = true;
	}

	async function handleCopy() {
		if (!(await ensurePlain())) return;
		// `plain` is a prop: after reveal() resolves the parent has updated it.
		await navigator.clipboard.writeText(plain ?? value);
		copied = true;
		setTimeout(() => (copied = false), 1500);
	}
</script>

<div class="flex items-center gap-1 rounded bg-muted pr-1">
	<code class="block min-w-0 flex-1 truncate px-3 py-2 font-mono text-xs" title={text}>
		{text}
	</code>
	{#if hasSecret && (plain || canReveal)}
		<Button
			variant="ghost"
			size="icon"
			class="size-7 shrink-0"
			disabled={busy}
			aria-label={shown ? 'Hide password' : 'Show password'}
			onclick={handleToggle}
		>
			{#if shown}<EyeOffIcon class="size-4" />{:else}<EyeIcon class="size-4" />{/if}
		</Button>
	{/if}
	{#if !hasSecret || plain || canReveal}
		<Button
			variant="ghost"
			size="icon"
			class="size-7 shrink-0"
			disabled={busy}
			aria-label="Copy connection string"
			onclick={handleCopy}
		>
			{#if copied}<CheckIcon class="size-4" />{:else}<CopyIcon class="size-4" />{/if}
		</Button>
	{/if}
</div>
