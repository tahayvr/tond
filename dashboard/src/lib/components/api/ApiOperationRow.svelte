<!--
  One API operation on the API page: method, path, summary and what it needs,
  expanding (native <details>) to its description, a curl example and the
  statuses it answers with.
-->
<script lang="ts">
	import { curlExample, type ApiOperation } from '$lib/api-reference';
	import { cn } from '$lib/utils';
	import ArrowRight from '~icons/ri/arrow-right-s-line';

	let { op, origin }: { op: ApiOperation; origin: string } = $props();

	const METHOD_TONE: Record<string, string> = {
		GET: 'text-info border-info/40',
		POST: 'text-success border-success/40',
		PUT: 'text-warn border-warn/40',
		PATCH: 'text-warn border-warn/40',
		DELETE: 'text-destructive border-destructive/40'
	};
</script>

<details class="group border-b last:border-b-0">
	<summary
		class="flex cursor-pointer list-none items-start gap-3 px-3 py-2.5 hover:bg-muted/50 focus-visible:outline-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden"
	>
		<ArrowRight
			class="mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90"
			aria-hidden="true"
		/>
		<span
			class={cn(
				'mt-px w-14 shrink-0 rounded-sm border px-1 py-px text-center font-mono text-[11px] font-semibold',
				METHOD_TONE[op.method]
			)}>{op.method}</span
		>
		<span class="min-w-0 flex-1 space-y-1">
			<span class="block font-mono text-sm break-all">{op.path}</span>
			<span class="block text-sm text-muted-foreground">{op.summary}</span>
			<span class="flex flex-wrap gap-1.5">
				{#if op.public}
					<span class="rounded-sm bg-muted px-1.5 py-px text-xs text-muted-foreground"
						>No token needed</span
					>
				{:else if op.scopes.length === 0}
					<span class="rounded-sm bg-muted px-1.5 py-px text-xs text-muted-foreground"
						>Any signed-in caller</span
					>
				{/if}
				{#each op.scopes as scope (scope)}
					<span class="rounded-sm border px-1.5 py-px font-mono text-xs" title="Required scope"
						>{scope}</span
					>
				{/each}
				{#if op.websocket}
					<span class="rounded-sm bg-info/10 px-1.5 py-px text-xs text-info">WebSocket</span>
				{/if}
				{#if op.idempotent}
					<span
						class="rounded-sm bg-muted px-1.5 py-px text-xs text-muted-foreground"
						title="Send an Idempotency-Key header to retry safely">Idempotency-Key</span
					>
				{/if}
			</span>
		</span>
	</summary>
	<div class="min-w-0 space-y-3 px-3 pt-1 pb-4 text-sm sm:pl-[5.75rem]">
		{#if op.description}
			<p class="wrap-anywhere text-muted-foreground">{op.description}</p>
		{/if}
		<pre
			class="overflow-x-auto rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs break-all whitespace-pre-wrap">{curlExample(
				op,
				origin
			)}</pre>
		<dl class="flex flex-wrap gap-x-6 gap-y-1 text-xs">
			<div class="flex gap-1.5">
				<dt class="text-muted-foreground">Success</dt>
				<dd class="font-mono">{op.status}</dd>
			</div>
			{#if op.errors.length > 0}
				<div class="flex gap-1.5">
					<dt class="text-muted-foreground">Errors</dt>
					<dd class="font-mono">{op.errors.join(', ')}</dd>
				</div>
			{/if}
			<div class="flex gap-1.5">
				<dt class="text-muted-foreground">Operation</dt>
				<dd class="font-mono break-all">{op.id}</dd>
			</div>
		</dl>
	</div>
</details>
