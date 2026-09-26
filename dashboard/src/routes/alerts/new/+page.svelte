<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		createAlertRule,
		type AlertKindInfo,
		type AlertRuleInput,
		type App,
		type Integration,
		type QuietHours
	} from '$lib/api';
	import { setUiContext, clearUiContext, alertsContext } from '$lib/ui-context.svelte';
	import PageHeader from '$lib/components/nav/page-header.svelte';
	import Breadcrumb from '$lib/components/nav/breadcrumb.svelte';
	import AlertRuleForm from '$lib/components/alerts/AlertRuleForm.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	let kinds = $derived(data.kinds as AlertKindInfo[]);
	let apps = $derived(data.apps as App[]);
	let channels = $derived(data.integrations as Integration[]);
	let quietHours = $derived((data.quietHours ?? undefined) as QuietHours | undefined);

	async function create(input: AlertRuleInput) {
		const rule = await createAlertRule(input);
		await goto(`/alerts/${rule.id}`, { invalidateAll: true });
	}

	$effect(() => {
		setUiContext(alertsContext());
		return () => clearUiContext();
	});
</script>

<div class="space-y-6 pb-12">
	<Breadcrumb items={[{ label: 'Alerts', href: '/alerts?tab=rules' }, { label: 'New rule' }]} />
	<PageHeader
		title="New alert rule"
		description="Watch one condition. Tond tells your channels when it fires and when it's resolved."
	/>
	<AlertRuleForm
		{kinds}
		{apps}
		{channels}
		globalQuiet={quietHours}
		submitLabel="Create rule"
		pendingLabel="Creating…"
		onsubmit={create}
	/>
</div>
