<script lang="ts">
	import { can } from '$lib/auth.svelte';
	import type { AlertRule } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import Bell from '~icons/ri/notification-3-line';
	import Plus from '~icons/ri/add-line';
	import AlertRuleGrid from '$lib/components/cards/alert-rule-card/alert-rule-grid.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
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
	<PageHeader
		title="Alerts"
		description={rules.length ? `${enabledCount} of ${rules.length} rules enabled` : undefined}
	>
		{#snippet actions()}
			{#if can('settings:write')}<Button href="/alerts/new">
					<Plus class="mr-1 h-4 w-4" />
					New alert rule
				</Button>{/if}
		{/snippet}
	</PageHeader>

	{#if rules.length === 0}
		<div
			class="flex flex-col items-center justify-center rounded-lg border border-border bg-card px-4 py-16 text-center"
		>
			<Bell aria-hidden="true" class="mb-4 h-12 w-12 text-muted-foreground" />
			<p class="font-medium">No alert rules yet</p>
			<p class="mt-2 text-sm text-muted-foreground">
				Get a message on Slack, Discord, Telegram or email when an app goes down or a deploy fails.
			</p>
			<Button class="mt-4" href="/alerts/new">
				<Plus class="mr-1 h-4 w-4" />
				New alert rule
			</Button>
		</div>
	{:else}
		<AlertRuleGrid {rules} />
	{/if}
</div>
