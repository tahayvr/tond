<!-- `?` overlay listing every keyboard shortcut. -->
<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { SHORTCUTS } from '$lib/dashboard-shortcuts';

	let { open = $bindable(false) }: { open?: boolean } = $props();
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>Keyboard shortcuts</Dialog.Title>
			<Dialog.Description>Shortcuts work anywhere except while typing in a field.</Dialog.Description>
		</Dialog.Header>
		<dl class="grid max-h-[60vh] gap-2 overflow-y-auto text-sm">
			{#each SHORTCUTS as shortcut (shortcut.description)}
				<div class="flex items-center justify-between gap-4">
					<dt class="text-muted-foreground">{shortcut.description}</dt>
					<dd class="flex shrink-0 gap-1">
						{#each shortcut.keys as key, i (i)}
							<kbd class="rounded-sm border bg-muted px-1.5 py-0.5 font-mono text-xs text-foreground"
								>{key}</kbd
							>
						{/each}
					</dd>
				</div>
			{/each}
		</dl>
	</Dialog.Content>
</Dialog.Root>
