<!--
  The engine version of a database and the guarded upgrade to a newer one.
  Downgrades aren't offered (the server refuses them too).
-->
<script lang="ts">
	import { upgradeDatabase, type ManagedDatabaseDetail } from '$lib/api';
	import { confirmAction } from '$lib/confirm.svelte';
	import { engineLabel, upgradeWarnings } from '$lib/database-ops';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import * as Select from '$lib/components/ui/select/index.js';
	import UpgradeIcon from '~icons/ri/arrow-up-circle-line';
	import LockIcon from '~icons/ri/lock-line';

	interface Props {
		db: ManagedDatabaseDetail;
		/** Admins start upgrades */
		canUpgrade: boolean;
		onstarted: (db: ManagedDatabaseDetail) => void;
	}

	let { db, canUpgrade, onstarted }: Props = $props();

	let target = $state('');
	$effect(() => {
		// Default to the newest version, and follow the database's changes.
		const targets = db.upgrade_targets;
		if (!targets.includes(target)) target = targets.at(-1) ?? '';
	});

	let stopped = $derived(db.status !== 'running');

	function handleUpgrade() {
		if (!target) return;
		const to = target;
		return confirmAction({
			title: `Upgrade ${db.name} to version ${to}?`,
			description: `From ${engineLabel(db)} to version ${to}. It usually takes a minute or two; large databases take longer.`,
			details: upgradeWarnings(db.db_type, db.version, to),
			confirmLabel: `Upgrade to ${to}`,
			destructive: false,
			pendingLabel: 'Starting…',
			onconfirm: async () => {
				const upgrade = await upgradeDatabase(db.id, to);
				onstarted({ ...db, upgrade, upgrade_targets: [] });
			}
		});
	}
</script>

<section class="max-w-2xl space-y-3" aria-labelledby="db-version-heading" data-testid="db-version">
	<div>
		<h2 id="db-version-heading" class="text-sm font-semibold">Version</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			{engineLabel(db)}
			{#if db.image_used}
				<span class="font-mono text-xs">({db.image_used})</span>
			{/if}
		</p>
	</div>

	{#if db.custom_image}
		<p class="text-sm text-muted-foreground">
			This database runs a custom image, so Tond doesn't manage its version. Clear the custom image on
			the Overview tab to upgrade here.
		</p>
	{:else if db.upgrade}
		<p class="text-sm text-muted-foreground">
			{db.upgrade.status === 'awaiting_confirmation'
				? 'Keep the new version or roll back (above) before upgrading again.'
				: db.upgrade.status === 'running'
					? 'An upgrade is running; see above.'
					: 'The last upgrade failed; see above. Dismiss it to try again.'}
		</p>
	{:else if db.upgrade_targets.length === 0}
		<p class="text-sm text-muted-foreground">
			{db.db_type === 'keydb'
				? "KeyDB always runs its latest image; there's nothing to upgrade."
				: 'This is the newest version Tond offers.'}
		</p>
	{:else}
		<div class="flex flex-wrap items-end gap-3">
			<div class="grid gap-2">
				<Label for="db-upgrade-target">Upgrade to</Label>
				<Select.Root type="single" bind:value={target} disabled={!canUpgrade}>
					<Select.Trigger id="db-upgrade-target" class="w-40" aria-label="Target version">
						Version {target}
					</Select.Trigger>
					<Select.Content>
						{#each db.upgrade_targets as v (v)}
							<Select.Item value={v}>Version {v}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
			</div>
			<Button
				variant="outline"
				size="sm"
				disabled={!canUpgrade || stopped || !target}
				onclick={handleUpgrade}
			>
				<UpgradeIcon class="size-4" />
				Upgrade…
			</Button>
		</div>
		<p class="text-xs text-muted-foreground">
			Apps lose their connection during the upgrade. A backup is taken first, and the old data is kept
			until you confirm, so you can roll back. Downgrades aren't possible.
		</p>
		{#if stopped}
			<p class="text-xs text-warn">Start the database to upgrade it.</p>
		{/if}
		{#if !canUpgrade}
			<p class="flex items-center gap-1.5 text-xs text-muted-foreground">
				<LockIcon class="size-3.5" /> Only admins can upgrade databases.
			</p>
		{/if}
	{/if}
</section>
