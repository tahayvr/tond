<!--
  AlertHistory — past and current firing episodes, filtered and paginated.

  <AlertHistory rules={rules} apps={apps} onopen={(id) => …} />
  <AlertHistory ruleId={rule.id} onopen={…} />   (one rule, no rule filter)
-->
<script lang="ts">
	import { listAlertHistory, type AlertHistoryPage, type AlertRule, type App } from '$lib/api';
	import { errorMessage } from '$lib/action.svelte';
	import {
		SEVERITY_LABEL,
		formatDuration,
		secondsBetween,
		severityTone,
		valueVsThreshold
	} from '$lib/alerts';
	import { formatRelativePast } from '$lib/time';
	import StatusBadge from '$lib/components/feedback/status-badge.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';

	const PER_PAGE = 20;

	let {
		rules = [],
		apps = [],
		ruleId,
		refreshKey = 0,
		onopen
	}: {
		rules?: AlertRule[];
		apps?: App[];
		/** Show only this rule's history (hides the rule filter) */
		ruleId?: string;
		/** Change to reload */
		refreshKey?: number;
		onopen?: (episodeId: string) => void;
	} = $props();

	let ruleFilter = $state('');
	let appFilter = $state('');
	let stateFilter = $state<'' | 'firing' | 'resolved'>('');
	let pageNo = $state(1);
	let result = $state<AlertHistoryPage | null>(null);
	let loading = $state(true);
	let error = $state('');

	let pages = $derived(result ? Math.max(1, Math.ceil(result.total / PER_PAGE)) : 1);
	let filtered = $derived(!!(ruleFilter || appFilter || stateFilter));

	$effect(() => {
		const filter = {
			rule_id: ruleId ?? (ruleFilter || undefined),
			app_id: appFilter || undefined,
			state: stateFilter || undefined,
			page: pageNo,
			per_page: PER_PAGE
		};
		void refreshKey;
		let cancelled = false;
		loading = true;
		error = '';
		listAlertHistory(filter)
			.then((r) => {
				if (!cancelled) result = r;
			})
			.catch((e) => {
				if (!cancelled) error = errorMessage(e, "The alert history couldn't be loaded. Try again.");
			})
			.finally(() => {
				if (!cancelled) loading = false;
			});
		return () => {
			cancelled = true;
		};
	});

	function setFilter(apply: () => void) {
		apply();
		pageNo = 1;
	}

	const STATE_LABEL = { '': 'Any state', firing: 'Firing', resolved: 'Resolved' } as const;
</script>

<div class="space-y-3">
	<div class="flex flex-wrap gap-2" role="group" aria-label="Filter the history">
		{#if !ruleId}
			<Select.Root
				type="single"
				value={ruleFilter || 'all'}
				onValueChange={(v) => setFilter(() => (ruleFilter = v === 'all' ? '' : (v ?? '')))}
			>
				<Select.Trigger class="w-full sm:w-52" aria-label="Rule">
					{rules.find((r) => r.id === ruleFilter)?.name ?? 'All rules'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="all">All rules</Select.Item>
					{#each rules as r (r.id)}
						<Select.Item value={r.id}>{r.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<Select.Root
				type="single"
				value={appFilter || 'all'}
				onValueChange={(v) => setFilter(() => (appFilter = v === 'all' ? '' : (v ?? '')))}
			>
				<Select.Trigger class="w-full sm:w-44" aria-label="App">
					{apps.find((a) => a.id === appFilter)?.name ?? 'All apps'}
				</Select.Trigger>
				<Select.Content>
					<Select.Item value="all">All apps</Select.Item>
					{#each apps as a (a.id)}
						<Select.Item value={a.id}>{a.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		{/if}
		<Select.Root
			type="single"
			value={stateFilter || 'any'}
			onValueChange={(v) =>
				setFilter(() => (stateFilter = v === 'firing' || v === 'resolved' ? v : ''))}
		>
			<Select.Trigger class="w-full sm:w-36" aria-label="State">
				{STATE_LABEL[stateFilter]}
			</Select.Trigger>
			<Select.Content>
				<Select.Item value="any">Any state</Select.Item>
				<Select.Item value="firing">Firing</Select.Item>
				<Select.Item value="resolved">Resolved</Select.Item>
			</Select.Content>
		</Select.Root>
	</div>

	<InlineError message={error} />

	{#if loading && !result}
		<div class="space-y-2">
			<Skeleton class="h-14 w-full" />
			<Skeleton class="h-14 w-full" />
		</div>
	{:else if result && result.episodes.length === 0}
		<div
			class="rounded-sm border border-dashed px-4 py-8 text-center text-sm text-muted-foreground"
		>
			{filtered ? 'No alerts match these filters.' : 'No alerts have fired yet.'}
		</div>
	{:else if result}
		<ul class="divide-y rounded-sm border bg-card" aria-busy={loading}>
			{#each result.episodes as ep (ep.id)}
				<li class="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
					<div class="min-w-0 space-y-1">
						<div class="flex flex-wrap items-center gap-2">
							{#if ep.state === 'firing'}
								<StatusBadge
									status={ep.severity}
									tone={severityTone(ep.severity)}
									label={`Firing · ${SEVERITY_LABEL[ep.severity]}`}
									size="sm"
								/>
							{:else}
								<StatusBadge status="success" label="Resolved" size="sm" />
							{/if}
							<span class="font-medium break-words">
								{ep.rule_name} <span class="text-muted-foreground">·</span>
								{ep.target_label}
							</span>
						</div>
						<p class="text-sm text-muted-foreground">
							{valueVsThreshold(ep)} · fired {formatRelativePast(ep.fired_at)}
							{#if ep.resolved_at}
								· lasted {formatDuration(secondsBetween(ep.fired_at, ep.resolved_at))}
							{/if}
						</p>
						<p class="text-xs text-muted-foreground">
							{ep.notifications_sent} notification{ep.notifications_sent === 1 ? '' : 's'} sent
							{#if ep.failed_deliveries > 0}
								· <span class="text-destructive">{ep.failed_deliveries} failed</span>
							{/if}
							{#if ep.acknowledged_by}
								· acknowledged by {ep.acknowledged_by}
							{/if}
						</p>
					</div>
					{#if onopen}
						<Button
							variant="ghost"
							size="sm"
							class="self-start sm:self-center"
							onclick={() => onopen?.(ep.id)}
						>
							Details
						</Button>
					{/if}
				</li>
			{/each}
		</ul>
		{#if pages > 1}
			<nav class="flex items-center justify-between gap-2 text-sm" aria-label="History pages">
				<Button
					variant="outline"
					size="sm"
					disabled={pageNo <= 1 || loading}
					onclick={() => (pageNo -= 1)}
				>
					Newer
				</Button>
				<span class="text-muted-foreground">Page {pageNo} of {pages} · {result.total} alerts</span>
				<Button
					variant="outline"
					size="sm"
					disabled={pageNo >= pages || loading}
					onclick={() => (pageNo += 1)}
				>
					Older
				</Button>
			</nav>
		{/if}
	{/if}
</div>
