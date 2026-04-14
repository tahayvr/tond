<script lang="ts">
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';

	interface Props {
		memoryLimit: number | null;
		memoryUnit: 'mb' | 'gb';
	}

	let { memoryLimit = $bindable(null), memoryUnit = $bindable('mb') }: Props = $props();
</script>

<div class="space-y-6">
	<div class="max-w-sm space-y-2">
		<Label>Memory Limit</Label>
		<div class="flex gap-2">
			<Input type="number" bind:value={memoryLimit} placeholder="Unlimited" min={1} />
			<Select.Root type="single" bind:value={memoryUnit}>
				<Select.Trigger class="w-24">
					{memoryUnit.toUpperCase()}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="mb">MB</Select.Item>
					<Select.Item value="gb">GB</Select.Item>
				</Select.Content>
			</Select.Root>
		</div>
		<p class="text-xs text-muted-foreground">
			Leave empty for unlimited memory. Recommended: 256 MB for PostgreSQL/MySQL, 128 MB for Redis.
		</p>
	</div>
</div>
