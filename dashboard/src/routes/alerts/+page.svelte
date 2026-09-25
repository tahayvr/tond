<script lang="ts">
	import type { AlertRule } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import { goto } from '$app/navigation';
	import Bell from '~icons/ri/notification-3-line';
	import Plus from '~icons/ri/add-line';
	import AlertRuleGrid from '$lib/components/cards/alert-rule-card/alert-rule-grid.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let rules = $derived(data.rules as AlertRule[]);

	let enabledCount = $derived(rules.filter((r) => r.enabled).length);

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<!-- Header -->
	<div class="flex h-14 items-center justify-between gap-4">
		<div>
			<h2 class="text-3xl font-extrabold uppercase">Alerts</h2>
			<p class="font-mono text-xs tracking-wider text-muted-foreground/70">
				{enabledCount}/{rules.length} MONITORS ACTIVE
			</p>
		</div>
		<Button onclick={() => goto('/alerts/new')}>
			<Plus class="mr-1 h-4 w-4" />
			New Alert Rule
		</Button>
	</div>

	{#if rules.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card py-16"
		>
			<Bell class="mb-4 h-12 w-12 text-muted-foreground/40" />
			<p class="font-mono text-[11px] tracking-widest text-muted-foreground/60 uppercase">
				NO_MONITORS_CONFIGURED
			</p>
			<p class="mt-2 text-sm text-muted-foreground/70">
				Create a rule to get notified when a threshold is crossed
			</p>
			<Button class="mt-4" onclick={() => goto('/alerts/new')}>
				<Plus class="mr-1 h-4 w-4" />
				New Alert Rule
			</Button>
		</div>
	{:else}
		<AlertRuleGrid {rules} />
	{/if}
</div>
