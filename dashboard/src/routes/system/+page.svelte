<script lang="ts">
	import { untrack } from 'svelte';
	import { page } from '$app/state';
	import {
		getHeartbeat,
		getHostOverview,
		type HeartbeatStatus,
		type HostOverview,
		type SystemMetrics
	} from '$lib/api';
	import { can } from '$lib/auth.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import { SYSTEM_TABS, tabFromUrl, tabHref, type SystemTab } from '$lib/tabs';
	import { setUiContext, clearUiContext, projectsContext } from '$lib/ui-context.svelte';
	import SystemStatsGrid from '$lib/components/system/system-stats-grid.svelte';
	import TondHealthPanel from '$lib/components/system/TondHealthPanel.svelte';
	import HostHistoryPanel from '$lib/components/system/HostHistoryPanel.svelte';
	import DiskPanel from '$lib/components/system/DiskPanel.svelte';
	import UptimeList from '$lib/components/uptime/UptimeList.svelte';
	import UptimePanel from '$lib/components/uptime/UptimePanel.svelte';
	import UpdatesPanel from '$lib/components/system/UpdatesPanel.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import LinkTabs from '$lib/components/nav/link-tabs.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let metrics = $derived(data.metrics as SystemMetrics | null);
	let tab = $derived<SystemTab>(tabFromUrl(page.url, SYSTEM_TABS, 'overview'));

	let overview = $state<HostOverview | null>(null);
	let heartbeat = $state<HeartbeatStatus | null>(null);
	let error = $state('');

	async function loadOverview() {
		try {
			const [o, h] = await Promise.all([getHostOverview(), getHeartbeat()]);
			overview = o;
			heartbeat = h;
			error = '';
		} catch (e: any) {
			error = `Couldn't load the host overview: ${e.message ?? 'request failed'}. It retries every 30 s.`;
		}
	}

	// Docker, heartbeat, forecast and disk breakdown: the Overview and Disk tabs.
	$effect(() => {
		if (tab !== 'overview' && tab !== 'disk') return;
		return pollWhileVisible(() => untrack(loadOverview), 30_000);
	});

	$effect(() => {
		setUiContext(projectsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6">
	<PageHeader
		title="System"
		section={tab === 'overview' ? undefined : SYSTEM_TABS.find((t) => t.id === tab)?.label}
		description="The server, its history and disk, Tond's services, uptime and updates."
	/>

	<LinkTabs
		label="System sections"
		tabs={SYSTEM_TABS}
		active={tab}
		href={(id) => tabHref(page.url.pathname, id, 'overview')}
	/>

	<InlineError message={error} ondismiss={() => (error = '')} />

	{#if tab === 'overview'}
		{#if metrics}
			<SystemStatsGrid {metrics} />
		{/if}
		<TondHealthPanel docker={overview?.docker ?? null} {heartbeat} />
	{:else if tab === 'history'}
		<HostHistoryPanel />
	{:else if tab === 'disk'}
		<DiskPanel {overview} canClean={can('settings:write')} onchange={loadOverview} />
	{:else if tab === 'uptime'}
		<UptimeList />
		<section class="space-y-1" aria-labelledby="dashboard-uptime-title">
			<h2 id="dashboard-uptime-title" class="text-lg font-bold">The dashboard</h2>
			<p class="text-sm text-muted-foreground">
				Tond checks its own public address too. If the whole server goes down this check can't tell
				you: set up the heartbeat in Settings for that.
			</p>
			<UptimePanel target={null} canEdit={can('settings:write')} />
		</section>
	{:else if tab === 'updates'}
		<UpdatesPanel canManage={can('settings:write')} />
	{/if}
</div>
