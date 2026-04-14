<script lang="ts">
	import AlertRuleCard from './alert-rule-card.svelte';
	import type { AlertRule } from '$lib/api';

	interface Props {
		rules: AlertRule[];
	}

	let { rules }: Props = $props();
</script>

<div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
	{#each rules as rule (rule.id)}
		<AlertRuleCard {rule} />
	{/each}
</div>
