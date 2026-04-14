<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';

	let {
		open = $bindable(false),
		title,
		description,
		confirmation,
		onconfirm
	}: {
		open: boolean;
		title: string;
		description: string;
		confirmation: string;
		onconfirm: () => Promise<void>;
	} = $props();

	let typed = $state('');
	let error = $state('');
	let deleting = $state(false);

	let matches = $derived(typed === confirmation);

	function reset() {
		typed = '';
		error = '';
		deleting = false;
	}

	async function handleConfirm() {
		if (!matches) return;
		deleting = true;
		error = '';
		try {
			await onconfirm();
			open = false;
			reset();
		} catch (e: any) {
			error = e.message;
			deleting = false;
		}
	}
</script>

<Dialog.Root
	bind:open
	onOpenChange={(isOpen) => {
		if (!isOpen) reset();
	}}
>
	<Dialog.Content class="sm:max-w-md">
		<Dialog.Header>
			<Dialog.Title>{title}</Dialog.Title>
			<Dialog.Description>{description}</Dialog.Description>
		</Dialog.Header>

		<div class="grid gap-4 py-2">
			<div class="grid gap-2">
				<Label for="confirm-input">
					Type <span class="font-semibold text-foreground">"{confirmation}"</span> to confirm
				</Label>
				<Input
					id="confirm-input"
					type="text"
					bind:value={typed}
					placeholder={confirmation}
					disabled={deleting}
					autocomplete="off"
				/>
			</div>

			{#if error}
				<div class="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</div>
			{/if}
		</div>

		<Dialog.Footer>
			<Button variant="outline" disabled={deleting} onclick={() => (open = false)}>Cancel</Button>
			<Button variant="destructive" disabled={!matches || deleting} onclick={handleConfirm}>
				{deleting ? 'Deleting…' : 'Delete'}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
