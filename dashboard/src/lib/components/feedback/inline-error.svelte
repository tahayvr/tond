<!--
  InlineError — the one error banner. Renders nothing when `message` is empty.

  <InlineError message={error} />
  <InlineError title="Couldn't save" message={error} ondismiss={() => (error = '')} />

  role="alert" (assertive live region), so screen readers announce it.
  Keep it next to the form or action that failed.
-->
<script lang="ts">
	import type { Snippet } from 'svelte';
	import ErrorIcon from '~icons/ri/error-warning-line';
	import CloseIcon from '~icons/ri/close-line';
	import { cn } from '$lib/utils';

	let {
		message,
		title,
		ondismiss,
		actions,
		class: className
	}: {
		message: string | null | undefined;
		title?: string;
		ondismiss?: () => void;
		/** Optional buttons (e.g. Retry) rendered under the message. */
		actions?: Snippet;
		class?: string;
	} = $props();
</script>

{#if message}
	<div
		role="alert"
		data-slot="inline-error"
		class={cn(
			'flex w-full items-start gap-3 rounded-sm border border-destructive/40 bg-destructive/10 px-3 py-2.5 text-sm text-destructive',
			className
		)}
	>
		<ErrorIcon aria-hidden="true" class="mt-0.5 size-4 shrink-0" />
		<div class="min-w-0 flex-1 space-y-1">
			{#if title}<p class="font-semibold">{title}</p>{/if}
			<!-- pre-line keeps multi-line server errors (e.g. compose problems) readable -->
			<p class="break-words whitespace-pre-line">{message}</p>
			{#if actions}<div class="flex flex-wrap gap-2 pt-1">{@render actions()}</div>{/if}
		</div>
		{#if ondismiss}
			<button
				type="button"
				onclick={ondismiss}
				class="-m-1 shrink-0 rounded-sm p-1 hover:bg-destructive/15"
				aria-label="Dismiss error"
			>
				<CloseIcon aria-hidden="true" class="size-4" />
			</button>
		{/if}
	</div>
{/if}
