<script lang="ts">
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import { listSnapshots, rollbackAppToSnapshot, type Snapshot } from '$lib/api';
	import { Button } from '$lib/components/ui/button/index.js';
	import RefreshCw from '~icons/ri/refresh-line';

	interface Props {
		appName: string;
	}

	let { appName }: Props = $props();

	// ── State ─────────────────────────────────────────────────────────────────

	let snapshots = $state<Snapshot[]>([]);
	let loading = $state(true);
	let error = $state('');
	let successMessage = $state('');
	let confirmId = $state<string | null>(null);
	let rollingBackId = $state<string | null>(null);

	// ── Load ──────────────────────────────────────────────────────────────────

	$effect(() => {
		load();
	});

	async function load() {
		loading = true;
		error = '';
		try {
			snapshots = await listSnapshots({ app: appName, trigger: 'pre_deploy' });
		} catch (e: any) {
			error = e.message ?? 'Failed to load snapshot history';
		} finally {
			loading = false;
		}
	}

	// ── Rollback ──────────────────────────────────────────────────────────────

	async function handleRollback(snap: Snapshot) {
		rollingBackId = snap.id;
		confirmId = null;
		error = '';
		successMessage = '';
		try {
			const result = await rollbackAppToSnapshot(snap.id, appName);
			successMessage = result.message;
		} catch (e: any) {
			error = e.message ?? 'Rollback failed';
		} finally {
			rollingBackId = null;
		}
	}

	// ── Helpers ───────────────────────────────────────────────────────────────

	function timeAgo(iso: string): string {
		const diff = Date.now() - new Date(iso).getTime();
		const mins = Math.floor(diff / 60_000);
		const hours = Math.floor(mins / 60);
		const days = Math.floor(hours / 24);
		if (days > 0) return `${days} day${days === 1 ? '' : 's'} ago`;
		if (hours > 0) return `${hours} hour${hours === 1 ? '' : 's'} ago`;
		if (mins > 0) return `${mins} minute${mins === 1 ? '' : 's'} ago`;
		return 'just now';
	}
</script>

<div class="space-y-4 pt-4">
	<p class="text-xs text-muted-foreground">
		Automatic snapshots taken before each deployment. Roll back to any point in seconds — no server
		restart required.
	</p>

	<InlineError title="History Error" message={error} />

	<SuccessAlert message={successMessage} />

	{#if loading}
		<p class="py-8 text-center text-xs text-muted-foreground">Loading history…</p>
	{:else if snapshots.length === 0}
		<div class="border border-dashed py-12 text-center text-xs text-muted-foreground">
			No pre-deploy snapshots for {appName} yet. They are created automatically before each deployment
			when the app has volumes.
		</div>
	{:else}
		<div class="space-y-px">
			{#each snapshots as snap (snap.id)}
				<div class="border bg-background text-xs">
					<div class="flex items-center justify-between px-4 py-3">
						<div class="flex min-w-0 flex-col gap-0.5">
							<div class="flex items-center gap-2">
								{#if snap.deployment_id}
									<span class="font-medium">Deploy #{snap.deployment_id}</span>
								{:else}
									<span class="font-medium">{snap.name}</span>
								{/if}
								<span class="text-muted-foreground">{timeAgo(snap.created_at)}</span>
							</div>
							{#if snap.status === 'failed' && snap.error_message}
								<span class="text-destructive">{snap.error_message}</span>
							{/if}
						</div>

						<div class="ml-4 flex shrink-0 items-center gap-1">
							{#if confirmId === snap.id}
								<div class="flex items-center gap-1">
									<span class="text-[10px] text-muted-foreground"
										>Roll {appName} back to before this deploy?</span
									>
									<Button
										size="sm"
										class="h-7 text-xs uppercase"
										disabled={rollingBackId === snap.id}
										onclick={() => handleRollback(snap)}
									>
										{rollingBackId === snap.id ? 'Rolling back…' : 'Confirm'}
									</Button>
									<Button
										variant="outline"
										size="sm"
										class="h-7 text-xs uppercase"
										onclick={() => (confirmId = null)}
									>
										Cancel
									</Button>
								</div>
							{:else}
								<Button
									variant="outline"
									size="sm"
									class="h-7 text-xs uppercase"
									disabled={snap.status !== 'completed'}
									onclick={() => (confirmId = snap.id)}
								>
									<RefreshCw class="h-3 w-3" />
									Rollback to before this
								</Button>
							{/if}
						</div>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</div>
