<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Button } from '$lib/components/ui/button/index.js';
	import Refresh from '~icons/ri/refresh-line';
	import Home from '~icons/ri/home-4-line';

	let retrying = $state(false);

	// ApiErrors thrown by load functions carry their HTTP status through
	// handleError (hooks.client.ts); 0 means the server never answered.
	let status = $derived(page.error?.status ?? page.status);
	let notFound = $derived(status === 404);
	let unavailable = $derived(status === 0 || status === 502 || status === 503 || status === 504);

	let title = $derived(
		notFound
			? 'Not found'
			: unavailable
				? 'Server unavailable'
				: status === 429
					? 'Too many requests'
					: "This page couldn't load"
	);

	async function handleRetry() {
		retrying = true;
		try {
			await goto(page.url, { invalidateAll: true, replaceState: true });
		} finally {
			retrying = false;
		}
	}
</script>

<div class="flex min-h-[60vh] w-full items-center justify-center p-6">
	<div class="w-full max-w-lg space-y-4 rounded-lg border border-border bg-card p-6" role="alert">
		<p class="font-mono text-[10px] tracking-[0.35em] text-muted-foreground/70 uppercase">
			{status ? `Error ${status}` : 'Connection error'}
		</p>
		<h1 class="text-2xl font-extrabold uppercase">{title}</h1>
		<p class="text-sm text-muted-foreground">
			{page.error?.message ?? 'Something went wrong.'}
		</p>
		{#if unavailable}
			<p class="text-xs text-muted-foreground">
				Tond may be restarting or updating. Nothing was lost — try again in a few seconds.
			</p>
		{/if}
		<div class="flex flex-wrap gap-2 pt-2">
			{#if !notFound}
				<Button onclick={handleRetry} disabled={retrying}>
					<Refresh class="mr-2 size-4" />
					{retrying ? 'Retrying…' : 'Retry'}
				</Button>
			{/if}
			<Button variant="outline" href="/">
				<Home class="mr-2 size-4" />
				Dashboard
			</Button>
		</div>
	</div>
</div>
