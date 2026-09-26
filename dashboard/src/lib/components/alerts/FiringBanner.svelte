<!--
  FiringBanner — shown on the home page while any alert is firing.

  <FiringBanner alerts={firing} />   (renders nothing when empty)
-->
<script lang="ts">
	import type { ActiveAlert } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import AlarmIcon from '~icons/ri/alarm-warning-line';
	import { cn } from '$lib/utils';

	let { alerts }: { alerts: ActiveAlert[] } = $props();

	let critical = $derived(alerts.some((a) => a.severity === 'critical'));
	let shown = $derived(alerts.slice(0, 3));
</script>

{#if alerts.length > 0}
	<section
		role="status"
		aria-labelledby="firing-banner-heading"
		class={cn(
			'flex flex-col gap-3 rounded-sm border px-4 py-3 sm:flex-row sm:items-center sm:justify-between',
			critical
				? 'border-destructive/40 bg-destructive/10 text-destructive'
				: 'border-warn/40 bg-warn/10 text-warn'
		)}
	>
		<div class="flex min-w-0 items-start gap-3">
			<AlarmIcon class="mt-0.5 size-5 shrink-0" aria-hidden="true" />
			<div class="min-w-0">
				<h2 id="firing-banner-heading" class="font-semibold">
					{alerts.length === 1 ? '1 alert is firing' : `${alerts.length} alerts are firing`}
				</h2>
				<ul class="text-sm">
					{#each shown as a (a.episode_id ?? `${a.rule_id}|${a.target}`)}
						<li class="break-words">
							<a href={`/alerts?alert=${a.episode_id}`} class="hover:underline">
								{a.rule_name} · {a.target_label}
							</a>
						</li>
					{/each}
					{#if alerts.length > shown.length}
						<li>and {alerts.length - shown.length} more</li>
					{/if}
				</ul>
			</div>
		</div>
		<Button
			variant="outline"
			size="sm"
			href="/alerts"
			class="shrink-0 bg-background text-foreground"
		>
			Open alerts
		</Button>
	</section>
{/if}
