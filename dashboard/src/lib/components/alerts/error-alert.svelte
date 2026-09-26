<!-- Deprecated: use InlineError from $lib/components/feedback directly. Kept as a thin alias. -->
<script lang="ts">
	import InlineError from '$lib/components/feedback/inline-error.svelte';

	let { title, description }: { title: string; description: string } = $props();
</script>

<InlineError title={title && title.toLowerCase() !== 'error' ? title : undefined} message={description} />
