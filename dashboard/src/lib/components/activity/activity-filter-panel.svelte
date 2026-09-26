<script lang="ts">
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import SearchIcon from '~icons/ri/search-line';
	import {
		ACTION_CATEGORIES,
		RESOURCE_TYPES,
		EMPTY_FILTERS,
		hasFilters,
		type ActivityFilters
	} from '$lib/activity-filters';

	interface Props {
		filters: ActivityFilters;
		/** Actors seen in the loaded entries, suggested in the actor field */
		knownActors: string[];
		loading: boolean;
		onApply: (filters: ActivityFilters) => void;
	}

	let { filters, knownActors, loading, onApply }: Props = $props();

	// Local draft: text fields apply on submit, choices apply immediately.
	let q = $state('');
	let actor = $state('');
	let action = $state('');
	let resourceType = $state('');
	let failuresOnly = $state(false);

	$effect(() => {
		q = filters.q;
		actor = filters.actor;
		action = filters.action;
		resourceType = filters.resourceType;
		failuresOnly = filters.failuresOnly;
	});

	function current(): ActivityFilters {
		return { q, actor, action, resourceType, failuresOnly };
	}

	function handleSubmit(e: Event) {
		e.preventDefault();
		onApply(current());
	}

	function handleClear() {
		onApply({ ...EMPTY_FILTERS });
	}
</script>

<form
	onsubmit={handleSubmit}
	class="grid grid-cols-2 gap-3 rounded-lg border border-border bg-card p-4 lg:grid-cols-[2fr_1.5fr_1fr_1fr_auto]"
	aria-label="Filter activity"
>
	<div class="col-span-2 grid gap-1.5 lg:col-span-1">
		<Label for="activity-q" class="text-xs text-muted-foreground">Search</Label>
		<div class="relative">
			<SearchIcon
				class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
			/>
			<Input
				id="activity-q"
				bind:value={q}
				placeholder="App, detail, action…"
				class="pl-8"
				autocomplete="off"
			/>
		</div>
	</div>
	<div class="col-span-2 grid gap-1.5 lg:col-span-1">
		<Label for="activity-actor" class="text-xs text-muted-foreground">Who</Label>
		<Input
			id="activity-actor"
			bind:value={actor}
			placeholder="Email or token name"
			list="activity-actors"
			autocomplete="off"
		/>
		<datalist id="activity-actors">
			{#each knownActors as a (a)}
				<option value={a}></option>
			{/each}
		</datalist>
	</div>
	<div class="grid gap-1.5">
		<Label for="activity-action" class="text-xs text-muted-foreground">What</Label>
		<Select.Root
			type="single"
			value={action}
			onValueChange={(v) => {
				action = v;
				onApply(current());
			}}
		>
			<Select.Trigger id="activity-action" class="w-full">
				{ACTION_CATEGORIES.find((c) => c.value === action)?.label ?? action}
			</Select.Trigger>
			<Select.Content>
				{#each ACTION_CATEGORIES as c (c.value)}
					<Select.Item value={c.value}>{c.label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>
	<div class="grid gap-1.5">
		<Label for="activity-resource" class="text-xs text-muted-foreground">On</Label>
		<Select.Root
			type="single"
			value={resourceType}
			onValueChange={(v) => {
				resourceType = v;
				onApply(current());
			}}
		>
			<Select.Trigger id="activity-resource" class="w-full">
				{RESOURCE_TYPES.find((r) => r.value === resourceType)?.label ?? resourceType}
			</Select.Trigger>
			<Select.Content>
				{#each RESOURCE_TYPES as r (r.value)}
					<Select.Item value={r.value}>{r.label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>
	<div class="col-span-2 flex items-end gap-2 lg:col-span-1">
		<Button type="submit" size="sm" disabled={loading}>{loading ? 'Filtering…' : 'Apply'}</Button>
		{#if hasFilters(filters)}
			<Button type="button" size="sm" variant="ghost" onclick={handleClear}>Clear</Button>
		{/if}
	</div>
	<label class="col-span-2 flex items-center gap-2 text-sm text-muted-foreground lg:col-span-5">
		<Checkbox
			checked={failuresOnly}
			onCheckedChange={(v) => {
				failuresOnly = v === true;
				onApply(current());
			}}
			aria-label="Only failures"
		/>
		Only failures (wrong passwords, refused sign-ins)
	</label>
</form>
