<script lang="ts">
	import {
		listIntegrations,
		listAlertIntegrations,
		linkAlertIntegration,
		unlinkAlertIntegration,
		type Integration,
		type AlertIntegrationLink
	} from '$lib/api';
	import { SvelteSet } from 'svelte/reactivity';
	import { Checkbox } from '$lib/components/ui/checkbox/index.js';
	import Plug from '~icons/ri/plug-line';

	interface Props {
		ruleId: string;
	}

	let { ruleId }: Props = $props();

	let integrations = $state<Integration[]>([]);
	let linked = $state<AlertIntegrationLink[]>([]);
	let loading = $state(true);
	let error = $state('');
	// Track in-flight toggles to prevent double-clicks
	let pending = new SvelteSet<string>();

	// Set of integration IDs currently linked to this rule
	let linkedIds = $derived(new Set(linked.map((l) => l.integration_id)));

	async function load() {
		loading = true;
		error = '';
		try {
			[integrations, linked] = await Promise.all([
				listIntegrations(),
				listAlertIntegrations(ruleId)
			]);
		} catch (e: any) {
			error = e.message ?? 'Failed to load integrations';
		} finally {
			loading = false;
		}
	}

	async function toggle(integration: Integration) {
		if (pending.has(integration.id)) return;
		pending.add(integration.id);
		error = '';
		try {
			if (linkedIds.has(integration.id)) {
				await unlinkAlertIntegration(ruleId, integration.id);
				linked = linked.filter((l) => l.integration_id !== integration.id);
			} else {
				const link = await linkAlertIntegration(ruleId, integration.id);
				linked = [
					...linked,
					{
						id: link.id,
						rule_id: link.rule_id,
						integration_id: link.integration_id,
						integration_name: integration.name,
						integration_kind: integration.kind,
						created_at: new Date().toISOString()
					}
				];
			}
		} catch (e: any) {
			error = e.message ?? 'Failed to update integration link';
		} finally {
			pending.delete(integration.id);
		}
	}

	function kindLabel(kind: string): string {
		const map: Record<string, string> = {
			telegram: 'Telegram',
			discord: 'Discord',
			slack: 'Slack / Mattermost',
			email: 'Email'
		};
		return map[kind] ?? kind;
	}

	$effect(() => {
		if (ruleId) load();
	});
</script>

<div class="grid gap-2">
	{#if loading}
		<p class="font-mono text-xs text-muted-foreground">Loading integrations...</p>
	{:else if error}
		<div
			class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-xs text-destructive"
		>
			{error}
		</div>
	{:else if integrations.length === 0}
		<div
			class="flex items-start gap-2 rounded border border-dashed border-border/60 p-3 opacity-60"
		>
			<Plug class="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
			<div>
				<p class="text-xs font-medium">No notification channels configured</p>
				<p class="mt-0.5 text-xs text-muted-foreground">
					Add Telegram, Discord, Slack, or Email integrations from the
					<a href="/integrations" class="underline underline-offset-2">Integrations</a> page.
				</p>
			</div>
		</div>
	{:else}
		<div class="space-y-2 rounded border border-border p-3">
			{#each integrations as integration (integration.id)}
				{@const isLinked = linkedIds.has(integration.id)}
				{@const isPending = pending.has(integration.id)}
				<div class="flex items-center gap-2">
					<Checkbox
						id={`int-${integration.id}`}
						checked={isLinked}
						disabled={isPending || !integration.enabled}
						onCheckedChange={() => toggle(integration)}
					/>
					<label
						for={`int-${integration.id}`}
						class="flex flex-1 cursor-pointer items-center justify-between gap-2 text-sm"
						class:opacity-40={!integration.enabled}
					>
						<span>{integration.name}</span>
						<span class="font-mono text-xs text-muted-foreground uppercase">
							{kindLabel(integration.kind)}
						</span>
					</label>
				</div>
			{/each}
		</div>
		<p class="text-xs text-muted-foreground">
			Selected channels will receive a notification when this rule fires.
			{#if integrations.some((i) => !i.enabled)}
				<span class="opacity-60">Disabled integrations cannot be selected.</span>
			{/if}
		</p>
	{/if}
</div>
