<script lang="ts">
	let {
		open = $bindable(false),
		onconfirm
	}: {
		open?: boolean;
		onconfirm: () => void | Promise<void>;
	} = $props();
</script>

{#if open}
	<button type="button" onclick={onconfirm}>Confirm delete</button>
{/if}
