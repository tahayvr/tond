<script lang="ts">
	import * as Alert from '$lib/components/ui/alert/index.js';
	import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';

	let { title, description }: { title: string; description: string } = $props();
</script>

<div class="grid w-full max-w-xl items-start gap-4">
	<Alert.Root>
		<AlertCircleIcon class="text-warn" />
		<Alert.Title class="text-warn">{title}</Alert.Title>
		<Alert.Description class="text-warn">{description}</Alert.Description>
	</Alert.Root>
</div>
