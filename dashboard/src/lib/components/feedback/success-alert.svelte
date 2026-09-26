<!--
  SuccessAlert — confirms a save or action. Renders nothing visible when `message` is empty.

  <SuccessAlert message={saved} />
  <SuccessAlert message={saved} timeout={4000} ondismiss={() => (saved = '')} />

  role="status" (polite live region, always mounted so updates are announced).
  With `timeout` + `ondismiss` it clears itself.
-->
<script lang="ts">
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import CloseIcon from '~icons/ri/close-line';
	import { cn } from '$lib/utils';

	let {
		message,
		ondismiss,
		timeout,
		class: className
	}: {
		message: string | null | undefined;
		ondismiss?: () => void;
		/** Auto-dismiss after this many ms (needs `ondismiss`). */
		timeout?: number;
		class?: string;
	} = $props();

	$effect(() => {
		if (!message || !timeout || !ondismiss) return;
		const dismiss = ondismiss;
		const timer = setTimeout(dismiss, timeout);
		return () => clearTimeout(timer);
	});
</script>

<div role="status" data-slot="success-alert" class={cn(!message && 'sr-only')}>
	{#if message}
		<div
			class={cn(
				'flex w-full items-start gap-3 rounded-sm border border-success/40 bg-success/10 px-3 py-2.5 text-sm text-success',
				className
			)}
		>
			<CheckIcon aria-hidden="true" class="mt-0.5 size-4 shrink-0" />
			<p class="min-w-0 flex-1 break-words">{message}</p>
			{#if ondismiss}
				<button
					type="button"
					onclick={ondismiss}
					class="-m-1 shrink-0 rounded-sm p-1 hover:bg-success/15"
					aria-label="Dismiss"
				>
					<CloseIcon aria-hidden="true" class="size-4" />
				</button>
			{/if}
		</div>
	{/if}
</div>
