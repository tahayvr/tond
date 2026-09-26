<script lang="ts">
	import { ALERT_GROUPS, type AlertRule } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { formatRelativePast } from '$lib/time';

	interface Props {
		rule: AlertRule;
	}

	let { rule }: Props = $props();

	let firedRecently = $derived(
		!!rule.last_fired_at && Date.now() - new Date(rule.last_fired_at).getTime() < 3_600_000
	);

	function metricLabel(metric: string | null): string {
		if (!metric) return '';
		for (const group of ALERT_GROUPS) {
			const m = group.metrics.find((x) => x.value === metric);
			if (m) return m.label;
		}
		return metric.toUpperCase();
	}

	function thresholdUnit(metric: string | null): string {
		return metric === 'app_down' ? 'min' : '%';
	}

	function conditionSummary(rule: AlertRule): string {
		if (rule.metric) {
			return `${metricLabel(rule.metric)} > ${rule.threshold}${thresholdUnit(rule.metric)}`;
		}
		if (rule.events.length > 0) {
			return `${rule.events.length} event${rule.events.length > 1 ? 's' : ''}`;
		}
		return 'No condition';
	}
</script>

<a href={`/alerts/${rule.id}`} class="block h-full rounded-sm" data-sveltekit-preload-data="hover">
	<Card.Root
		class="flex h-full w-full flex-col gap-3 rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="gap-2 p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Title class="min-w-0 truncate text-base">{rule.name}</Card.Title>
				{#if !rule.enabled}
					<StatusBadge status="paused" size="sm" />
				{:else if firedRecently}
					<StatusBadge status="firing" tone="warning" label="Fired recently" size="sm" />
				{:else}
					<StatusBadge status="enabled" size="sm" />
				{/if}
			</div>
			<Card.Description class="text-xs">{conditionSummary(rule)}</Card.Description>
		</Card.Header>
		<Card.Content
			class="flex grow items-end justify-between gap-2 p-0 text-xs text-muted-foreground"
		>
			<span>{rule.app_id ? 'One app' : 'All apps'}</span>
			<span>Last fired: {formatRelativePast(rule.last_fired_at)}</span>
		</Card.Content>
	</Card.Root>
</a>
