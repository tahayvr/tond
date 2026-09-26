<!-- Per-route document title: <PageTitle title={app.name} section="Logs" /> → "Logs · api · Tond" -->
<script lang="ts">
	import { formatTitle } from './page-title';

	let { title, section }: { title?: string | null; section?: string | null } = $props();
</script>

<svelte:head>
	<title>{formatTitle(title, section)}</title>
</svelte:head>
