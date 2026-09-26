<script lang="ts">
	import type { PasswordLink } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import CopyIcon from '~icons/ri/file-copy-line';
	import CheckIcon from '~icons/ri/check-line';

	let { link }: { link: PasswordLink } = $props();

	let copied = $state(false);
	const url = $derived(
		typeof window === 'undefined' ? link.path : `${window.location.origin}${link.path}`
	);

	async function handleCopy() {
		await navigator.clipboard.writeText(url);
		copied = true;
		setTimeout(() => (copied = false), 2000);
	}
</script>

<div class="space-y-2">
	<div class="flex items-center gap-2">
		<code
			class="min-w-0 flex-1 rounded-md border border-border bg-muted/40 px-3 py-2 font-mono text-xs break-all select-all"
			data-testid="password-link">{url}</code
		>
		<Button variant="outline" size="sm" onclick={handleCopy} aria-label="Copy link">
			{#if copied}<CheckIcon class="size-4" /> Copied{:else}<CopyIcon class="size-4" /> Copy{/if}
		</Button>
	</div>
	<p class="text-xs text-muted-foreground">
		Works once, until {new Date(link.expires_at).toLocaleString()}. Tond doesn't send email: share
		it over a channel you trust.
	</p>
</div>
