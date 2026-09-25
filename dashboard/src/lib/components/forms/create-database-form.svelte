<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Database from '~icons/ri/database-2-line';

	interface Props {
		name: string;
		dbType: string;
		projectId: string;
		memoryMb: string;
		error: string;
		isCreating?: boolean;
		onSubmit: () => void;
		onCancel: () => void;
	}

	let {
		name = $bindable(''),
		dbType = $bindable('postgres'),
		projectId = $bindable(''),
		memoryMb = $bindable(''),
		error,
		isCreating = false,
		onSubmit,
		onCancel
	}: Props = $props();
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-4 flex items-center gap-2">
		<Database class="h-4 w-4 text-muted-foreground/70" />
		<span class="font-mono text-[10px] tracking-wider text-muted-foreground/70 uppercase">
			INITIALIZE_DATASTORE
		</span>
	</div>
	<div class="grid gap-4 sm:grid-cols-2">
		<div class="flex flex-col gap-1.5">
			<Label for="create-name" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Name</Label
			>
			<Input
				id="create-name"
				bind:value={name}
				placeholder="my-db"
				class="border-border bg-card font-mono text-sm"
			/>
		</div>
		<div class="flex flex-col gap-1.5">
			<Label for="create-type" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Type</Label
			>
			<Select.Root type="single" bind:value={dbType}>
				<Select.Trigger id="create-type" class="border-border bg-card font-mono text-sm">
					{dbType || 'Select type'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="postgres">POSTGRES</Select.Item>
					<Select.Item value="mysql">MYSQL</Select.Item>
					<Select.Item value="redis">REDIS</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<div class="flex flex-col gap-1.5">
			<Label for="create-project" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Project ID (optional)</Label
			>
			<Input
				id="create-project"
				bind:value={projectId}
				placeholder="proj_…"
				class="border-border bg-card font-mono text-sm"
			/>
		</div>
		<div class="flex flex-col gap-1.5">
			<Label for="create-memory" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Memory limit (MiB)</Label
			>
			<Input
				id="create-memory"
				type="number"
				bind:value={memoryMb}
				placeholder="256"
				class="border-border bg-card font-mono text-sm"
			/>
		</div>
	</div>
	{#if error}
		<p class="mt-3 font-mono text-sm text-destructive">{error}</p>
	{/if}
	<div class="mt-4 flex justify-end gap-2">
		<Button variant="outline" onclick={onCancel}>Cancel</Button>
		<Button onclick={onSubmit} disabled={isCreating}>
			{isCreating ? 'Creating…' : 'Create Database'}
		</Button>
	</div>
</div>
