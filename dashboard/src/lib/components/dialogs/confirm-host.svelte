<!-- Renders requests made with confirmAction(). Mounted once in the root layout. -->
<script lang="ts">
	import ConfirmDialog from './confirm-dialog.svelte';
	import { getPendingConfirm, settleConfirm } from '$lib/confirm.svelte';

	let request = $derived(getPendingConfirm());
</script>

{#if request}
	{@const req = request}
	{#key req.id}
		<ConfirmDialog
			open={true}
			title={req.title}
			description={req.description}
			confirmLabel={req.confirmLabel ?? 'Confirm'}
			pendingLabel={req.pendingLabel}
			destructive={req.destructive ?? true}
			typedConfirmation={req.typedConfirmation}
			onconfirm={async () => {
				await req.onconfirm?.();
				settleConfirm(req.id, true);
			}}
			oncancel={() => settleConfirm(req.id, false)}
			details={req.details?.length ? detailList : undefined}
		/>
		{#snippet detailList()}
			<ul class="list-disc space-y-1 pl-5">
				{#each req.details ?? [] as item (item)}
					<li>{item}</li>
				{/each}
			</ul>
		{/snippet}
	{/key}
{/if}
