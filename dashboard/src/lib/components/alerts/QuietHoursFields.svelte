<!--
  QuietHoursFields — start, end, time zone and critical bypass of a quiet
  hours window.

  <QuietHoursFields idPrefix="global" bind:value={quiet} />
-->
<script lang="ts">
	import type { QuietHours } from '$lib/api';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';

	let {
		idPrefix,
		value = $bindable(),
		disabled = false
	}: { idPrefix: string; value: QuietHours; disabled?: boolean } = $props();

	const zones: string[] = (() => {
		try {
			return (
				(Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.(
					'timeZone'
				) ?? []
			);
		} catch {
			return [];
		}
	})();
</script>

<div class="grid gap-3 sm:grid-cols-3">
	<div class="grid gap-1.5">
		<Label for={`${idPrefix}-start`}>From</Label>
		<Input id={`${idPrefix}-start`} type="time" bind:value={value.start} {disabled} required />
	</div>
	<div class="grid gap-1.5">
		<Label for={`${idPrefix}-end`}>Until</Label>
		<Input id={`${idPrefix}-end`} type="time" bind:value={value.end} {disabled} required />
	</div>
	<div class="grid gap-1.5">
		<Label for={`${idPrefix}-tz`}>Time zone</Label>
		<Input
			id={`${idPrefix}-tz`}
			list={`${idPrefix}-zones`}
			bind:value={value.timezone}
			placeholder="Europe/Berlin"
			autocomplete="off"
			{disabled}
			required
		/>
		<datalist id={`${idPrefix}-zones`}>
			{#each zones as zone (zone)}
				<option value={zone}></option>
			{/each}
		</datalist>
	</div>
</div>
<div class="flex items-start gap-2">
	<Checkbox
		id={`${idPrefix}-bypass`}
		checked={value.critical_bypass}
		onCheckedChange={(c) => (value.critical_bypass = c === true)}
		{disabled}
	/>
	<div class="grid gap-0.5">
		<Label for={`${idPrefix}-bypass`}>Critical alerts come through anyway</Label>
		<p class="text-xs text-muted-foreground">
			Everything else waits until the window ends, and is dropped if it resolved meanwhile.
		</p>
	</div>
</div>
