<script lang="ts">
	import type { AlertRule } from '$lib/api';
	import { SEVERITY_LABEL, conditionSummary, kindLabel } from '$lib/alerts';
	import * as Card from '$lib/components/ui/card/index.js';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import { formatRelativePast } from '$lib/time';

	interface Props {
		rule: AlertRule;
	}

	let { rule }: Props = $props();

	let muted = $derived(!!rule.snoozed_until && new Date(rule.snoozed_until).getTime() > Date.now());
	let scope = $derived.by(() => {
		switch (rule.kind) {
			case 'disk_usage':
			case 'disk_forecast':
				return 'Server';
			case 'cert_expiry':
				return 'Every certificate';
			case 'backup_failed':
				return 'Backups';
			case 'cron_failed':
				return 'Every cron job';
			default:
				return rule.app_name ?? (rule.app_id ? 'One app' : 'Every app');
		}
	});
</script>

<a href={`/alerts/${rule.id}`} class="block h-full rounded-sm" data-sveltekit-preload-data="hover">
	<Card.Root
		class="flex h-full w-full flex-col gap-3 rounded-sm border p-5 transition-colors hover:border-foreground/40"
	>
		<Card.Header class="gap-2 p-0">
			<div class="flex items-start justify-between gap-2">
				<Card.Title class="min-w-0 text-base break-words">{rule.name}</Card.Title>
				{#if !rule.enabled}
					<StatusBadge status="paused" size="sm" />
				{:else if rule.firing > 0}
					<StatusBadge
						status="firing"
						label={rule.firing > 1 ? `${rule.firing} firing` : 'Firing'}
						size="sm"
					/>
				{:else if muted}
					<StatusBadge status="paused" label="Muted" size="sm" />
				{:else if rule.pending > 0}
					<StatusBadge status="pending" label="Pending" size="sm" />
				{:else}
					<StatusBadge status="success" label="OK" size="sm" />
				{/if}
			</div>
			<Card.Description class="text-xs">{conditionSummary(rule)}</Card.Description>
		</Card.Header>
		<Card.Content
			class="flex grow flex-wrap items-end justify-between gap-2 p-0 text-xs text-muted-foreground"
		>
			<span>{scope} · {SEVERITY_LABEL[rule.severity] ?? rule.severity}</span>
			<span title={kindLabel(rule.kind)}>Last fired {formatRelativePast(rule.last_fired_at)}</span>
		</Card.Content>
	</Card.Root>
</a>
