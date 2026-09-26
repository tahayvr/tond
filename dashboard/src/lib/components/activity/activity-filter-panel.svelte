<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import Activity from '~icons/ri/pulse-line';
	import type { App } from '$lib/api';

	interface Props {
		apps: App[];
		filterAppId: string;
		filterAction: string;
		filterLimit: number;
		onApply: () => void;
	}

	let {
		apps,
		filterAppId = $bindable(''),
		filterAction = $bindable(''),
		filterLimit = $bindable(50),
		onApply
	}: Props = $props();

	const actionTypes = [
		{ value: '', label: 'ALL_ACTIONS' },
		{ value: 'create', label: 'CREATE' },
		{ value: 'deploy', label: 'DEPLOY' },
		{ value: 'stop', label: 'STOP' },
		{ value: 'start', label: 'START' },
		{ value: 'delete', label: 'DELETE' },
		{ value: 'update', label: 'UPDATE' },
		{ value: 'rollback', label: 'ROLLBACK' }
	];
</script>

<div class="rounded-lg border border-border bg-card p-4">
	<div class="mb-4 flex items-center gap-2">
		<Activity class="h-4 w-4 text-muted-foreground" />
		<span class="font-mono text-[10px] tracking-wider text-muted-foreground uppercase">
			FILTER_PARAMETERS
		</span>
	</div>
	<div class="flex flex-wrap items-end gap-4">
		<div class="grid gap-1.5">
			<Label for="filter-app" class="font-mono text-[10px] text-muted-foreground uppercase"
				>App</Label
			>
			<Select.Root type="single" bind:value={filterAppId}>
				<Select.Trigger id="filter-app" class="w-50 border-border bg-card font-mono text-sm">
					{apps.find((a) => a.id === filterAppId)?.name ?? 'ALL_APPS'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="">ALL_APPS</Select.Item>
					{#each apps as a (a.id)}
						<Select.Item value={a.id}>{a.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="grid gap-1.5">
			<Label for="filter-action" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Action</Label
			>
			<Select.Root type="single" bind:value={filterAction}>
				<Select.Trigger id="filter-action" class="w-40 border-border bg-card font-mono text-sm">
					{actionTypes.find((a) => a.value === filterAction)?.label ?? 'ALL_ACTIONS'}
				</Select.Trigger>
				<Select.Content>
					{#each actionTypes as action (action.value)}
						<Select.Item value={action.value}>{action.label}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>
		<div class="grid gap-1.5">
			<Label for="filter-limit" class="font-mono text-[10px] text-muted-foreground uppercase"
				>Limit</Label
			>
			<Input
				id="filter-limit"
				type="number"
				bind:value={filterLimit}
				min={1}
				max={200}
				class="w-24 border-border bg-card font-mono text-sm"
			/>
		</div>
		<Button size="sm" onclick={onApply} class="font-mono text-[10px] uppercase">Apply</Button>
	</div>
</div>
