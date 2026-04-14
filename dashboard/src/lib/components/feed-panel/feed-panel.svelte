<script lang="ts">
	import * as Sidebar from '$lib/components/ui/sidebar/index.js';
	import FeedContent from './feed-content.svelte';
	import FeedHeader from './feed-header.svelte';
</script>

<Sidebar.Root side="right" collapsible="icon">
	<Sidebar.Header class="border-b p-4">
		<FeedHeader />
	</Sidebar.Header>
	<Sidebar.Content class="group-data-[collapsible=icon]:hidden">
		<div class="h-full">
			<FeedContent />
		</div>
	</Sidebar.Content>
	<Sidebar.Footer></Sidebar.Footer>
	<Sidebar.Rail />
</Sidebar.Root>
