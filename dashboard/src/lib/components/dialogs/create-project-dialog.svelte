<script lang="ts">
	import { createProject } from '$lib/api';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { goto } from '$app/navigation';

	let { open = $bindable(false) }: { open: boolean } = $props();

	let name = $state('');
	let description = $state('');
	let error = $state('');
	let submitting = $state(false);

	function reset() {
		name = '';
		description = '';
		error = '';
		submitting = false;
	}

	async function handleSubmit() {
		submitting = true;
		error = '';
		try {
			const project = await createProject({
				name: name.trim(),
				description: description.trim() || undefined
			});
			open = false;
			reset();
			goto(`/projects/${project.id}`);
		} catch (e: any) {
			error = e.message;
			submitting = false;
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
			<Dialog.Title>Create New Project</Dialog.Title>
		</Dialog.Header>
		<form
			onsubmit={(e) => {
				e.preventDefault();
				handleSubmit();
			}}
			class="grid gap-4 py-2"
		>
			<div class="grid gap-2">
				<Label for="project-name" class="uppercase"
					>Project Name: <span class="text-destructive">*</span></Label
				>
				<Input
					id="project-name"
					type="text"
					bind:value={name}
					placeholder="my-project"
					required
					disabled={submitting}
				/>
			</div>
			<div class="grid gap-2">
				<Label for="project-description" class="uppercase">Description:</Label>
				<Textarea
					id="project-description"
					bind:value={description}
					placeholder="What is this project for?"
					disabled={submitting}
				/>
			</div>
			{#if error}
				<div class="rounded-md bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</div>
			{/if}
			<Dialog.Footer>
				<Button type="submit" disabled={!name.trim() || submitting}>
					{submitting ? 'Creating…' : 'Create Project'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
