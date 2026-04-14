<script lang="ts">
	import { goto } from '$app/navigation';
	import { cn } from '$lib/utils';
	import { ALERT_GROUPS, type AlertRule } from '$lib/api';
	import * as Card from '$lib/components/ui/card/index.js';

	interface Props {
		rule: AlertRule;
	}

	let { rule }: Props = $props();

	/**
	 * Status dot colour:
	 *   disabled            → muted grey
	 *   fired within 1 hour → amber pulse
	 *   enabled + not recent → green
	 */
	function statusDotClass(rule: AlertRule): string {
		if (!rule.enabled) return 'bg-muted-foreground/30';
		if (rule.last_fired_at) {
			const diff = Date.now() - new Date(rule.last_fired_at).getTime();
			if (diff < 3_600_000) return 'bg-warn animate-pulse';
		}
		return 'bg-accent';
	}

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

	function formatRelative(dt: string | null): string {
		if (!dt) return 'Never';
		const diff = Date.now() - new Date(dt).getTime();
		const minutes = Math.floor(diff / 60_000);
		const hours = Math.floor(diff / 3_600_000);
		const days = Math.floor(diff / 86_400_000);
		if (minutes < 1) return 'just now';
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
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

<button
	type="button"
	class="block w-full text-left hover:cursor-pointer"
	onclick={() => goto(`/alerts/${rule.id}`)}
>
	<Card.Root
		class={cn(
			'flex w-full flex-col rounded-sm border-none p-5 transition-colors hover:border',
			!rule.enabled && 'opacity-60'
		)}
	>
		<Card.Header class="p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Description class="text-xs uppercase">id: {rule.id.slice(0, 8)}</Card.Description>
				<span class="font-mono text-[10px] text-muted-foreground/50 uppercase">
					{rule.enabled ? 'enabled' : 'paused'}
				</span>
			</div>
			<Card.Title class="text-base uppercase">{rule.name}</Card.Title>
			<div class="flex items-center gap-2">
				<span class="inline-block size-2 shrink-0 rounded-full {statusDotClass(rule)}"></span>
				<span class="font-mono text-[10px] tracking-wider text-secondary uppercase">
					{conditionSummary(rule)}
				</span>
			</div>
		</Card.Header>
		<Card.Content class="grow p-0">
			<div class="flex flex-col justify-center gap-2 py-4">
				<p class="text-center font-mono text-[10px] text-muted-foreground/70 uppercase">
					{rule.app_id ? `app: ${rule.app_id.slice(0, 8)}` : 'scope: global'}
				</p>
			</div>
		</Card.Content>
		<Card.Footer class="w-full justify-between p-0">
			<span class="font-mono text-[10px] text-muted-foreground/50">
				FIRED: {formatRelative(rule.last_fired_at)}
			</span>
		</Card.Footer>
	</Card.Root>
</button>
