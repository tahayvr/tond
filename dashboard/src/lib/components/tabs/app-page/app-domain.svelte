<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';

	interface Props {
		domainInput: string;
		onUpdate: () => void;
	}

	let { domainInput = $bindable(''), onUpdate }: Props = $props();
</script>

<div class="space-y-4 pt-4">
	<p class="text-sm text-muted-foreground">
		Set a custom domain for this app. Make sure your DNS points to this server.
	</p>
	<div class="flex gap-3">
		<div class="grid flex-1 gap-2">
			<Label for="domain-input">Domain</Label>
			<Input id="domain-input" type="text" bind:value={domainInput} placeholder="app.example.com" />
		</div>
		<Button onclick={onUpdate} class="self-end">Update Domain</Button>
	</div>
</div>
