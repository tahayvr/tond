<script lang="ts">
	import * as Alert from '$lib/components/ui/alert/index.js';
	import AlertCircleIcon from '~icons/ri/error-warning-line';

	let { title, description }: { title: string; description: string } = $props();
</script>

<div class="grid w-full max-w-xl items-start gap-4">
	<Alert.Root variant="destructive">
		<AlertCircleIcon />
		<Alert.Title>{title}</Alert.Title>
		<Alert.Description>{description}</Alert.Description>
	</Alert.Root>
</div>
