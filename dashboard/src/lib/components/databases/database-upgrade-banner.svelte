<!--
  The state of a major-version upgrade, above every tab: progress while it
  runs (polled), Confirm / Roll back once the new version runs, the reason
  when it failed. Renders nothing without an upgrade.
-->
<script lang="ts">
	import { untrack } from 'svelte';
	import {
		confirmDatabaseUpgrade,
		dismissDatabaseUpgrade,
		getDatabase,
		rollbackDatabaseUpgrade,
		type ManagedDatabaseDetail
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { pollWhileVisible } from '$lib/polling';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import LoaderIcon from '~icons/ri/loader-4-line';
	import CheckIcon from '~icons/ri/checkbox-circle-line';
	import ErrorIcon from '~icons/ri/error-warning-line';

	interface Props {
		db: ManagedDatabaseDetail;
		/** Admins can confirm and roll back */
		canManage: boolean;
		/** Called with the refreshed database (while polling, after actions) */
		onchange: (db: ManagedDatabaseDetail) => void;
	}

	let { db, canManage, onchange }: Props = $props();

	let upgrade = $derived(db.upgrade);
	let running = $derived(upgrade?.status === 'running');

	// Follow a running upgrade (every 2 s while the tab is visible).
	$effect(() => {
		if (!running) return;
		const id = db.id;
		return pollWhileVisible(
			() =>
				untrack(async () => {
					onchange(await getDatabase(id));
				}),
			2000,
			{ immediate: false }
		);
	});

	let message = $state('');

	async function refresh() {
		onchange(await getDatabase(db.id));
	}

	const confirm = useAction(() => confirmDatabaseUpgrade(db.id), {
		onSuccess: async (r) => {
			message = r.message;
			await refresh();
		}
	});
	const dismiss = useAction(() => dismissDatabaseUpgrade(db.id), { onSuccess: refresh });

	function handleConfirm() {
		if (!upgrade) return;
		return confirmAction({
			title: `Keep version ${upgrade.to_version}?`,
			description: `The version ${upgrade.from_version} data directory is deleted. After this the upgrade can't be rolled back; a backup from before the upgrade stays in Backups.`,
			confirmLabel: `Keep version ${upgrade.to_version}`,
			destructive: false,
			pendingLabel: 'Confirming…',
			onconfirm: () => confirm.run()
		});
	}

	function handleRollback() {
		if (!upgrade) return;
		const u = upgrade;
		return confirmAction({
			title: `Roll back to version ${u.from_version}?`,
			description: `${db.name} restarts on version ${u.from_version} with its data from before the upgrade.`,
			details: [
				`Anything written since the upgrade finished is lost.`,
				'Apps lose their connection for a few seconds while the database restarts.'
			],
			confirmLabel: `Roll back to ${u.from_version}`,
			pendingLabel: 'Rolling back…',
			destructive: true,
			onconfirm: async () => {
				const r = await rollbackDatabaseUpgrade(db.id);
				message = r.message;
				await refresh();
			}
		});
	}
</script>

{#if upgrade}
	<section
		class="space-y-3 rounded-lg border p-4"
		class:border-warn={upgrade.status === 'awaiting_confirmation'}
		class:border-destructive={upgrade.status === 'failed'}
		aria-live="polite"
		data-testid="database-upgrade"
	>
		{#if upgrade.status === 'running'}
			<div class="flex items-start gap-3">
				<LoaderIcon class="mt-0.5 size-5 shrink-0 animate-spin text-muted-foreground" />
				<div class="space-y-1">
					<p class="text-sm font-medium">
						Upgrading {db.name} from version {upgrade.from_version} to {upgrade.to_version}
					</p>
					<p class="text-sm text-muted-foreground">{upgrade.step}…</p>
					<p class="text-xs text-muted-foreground">
						Apps can't reach the database until the upgrade finishes. You can leave this page.
					</p>
				</div>
			</div>
		{:else if upgrade.status === 'awaiting_confirmation'}
			<div class="flex items-start gap-3">
				<CheckIcon class="mt-0.5 size-5 shrink-0 text-success" />
				<div class="min-w-0 space-y-1">
					<p class="text-sm font-medium">
						{db.name} now runs version {upgrade.to_version}
					</p>
					<p class="text-sm text-muted-foreground">
						Check that your apps work. Keep the new version to delete the version {upgrade.from_version}
						data, or roll back to it (writes since the upgrade are lost).
					</p>
				</div>
			</div>
			{#if canManage}
				<div class="flex flex-wrap gap-2">
					<Button size="sm" disabled={confirm.pending} onclick={handleConfirm}>
						Keep version {upgrade.to_version}
					</Button>
					<Button size="sm" variant="outline" onclick={handleRollback}>
						Roll back to {upgrade.from_version}
					</Button>
				</div>
			{:else}
				<p class="text-xs text-muted-foreground">An admin can confirm or roll back.</p>
			{/if}
			<InlineError message={confirm.error} ondismiss={confirm.clear} />
		{:else}
			<div class="flex items-start gap-3">
				<ErrorIcon class="mt-0.5 size-5 shrink-0 text-destructive" />
				<div class="min-w-0 space-y-1">
					<p class="text-sm font-medium">
						The upgrade to version {upgrade.to_version} didn't finish
					</p>
					<p class="text-sm break-words text-muted-foreground">
						{upgrade.error ?? 'See the server logs for details.'}
					</p>
				</div>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button size="sm" variant="outline" disabled={dismiss.pending} onclick={() => dismiss.run()}>
					{dismiss.pending ? 'Dismissing…' : 'Dismiss'}
				</Button>
			</div>
		{/if}
	</section>
{/if}

{#if message && !upgrade}
	<div
		class="flex items-start justify-between gap-3 rounded-md bg-success/10 px-3 py-2 text-sm text-success"
		role="status"
	>
		<span>{message}</span>
		<button type="button" class="text-xs underline" onclick={() => (message = '')}>Dismiss</button>
	</div>
{/if}
