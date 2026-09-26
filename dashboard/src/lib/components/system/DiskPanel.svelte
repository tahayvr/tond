<!--
  DiskPanel — what fills the disk (Docker images, build cache, containers,
  volumes, Tond's data, everything else), the disk-full forecast and a
  conservative "Free up space" action: unused build cache and dangling images
  only, previewed in a confirmation that lists exactly what goes.
-->
<script lang="ts">
	import {
		freeUpSpace,
		getCleanupPreview,
		refreshDiskUsage,
		type CleanupPreview,
		type DiskUsage,
		type HostOverview
	} from '$lib/api';
	import { useAction } from '$lib/action.svelte';
	import { confirmAction } from '$lib/confirm.svelte';
	import { diskSegments, forecastSummary } from '$lib/host-format';
	import { formatBytes } from '$lib/metrics-format';
	import { relativeUnix } from '$lib/uptime';
	import { cn } from '$lib/utils';
	import InlineError from '$lib/components/feedback/inline-error.svelte';
	import SuccessAlert from '$lib/components/feedback/success-alert.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Skeleton } from '$lib/components/ui/skeleton/index.js';
	import HardDrive from '~icons/ri/hard-drive-3-line';
	import RefreshIcon from '~icons/ri/refresh-line';
	import BroomIcon from '~icons/ri/brush-3-line';
	import TrendIcon from '~icons/ri/line-chart-line';

	interface Props {
		overview: HostOverview | null;
		canClean: boolean;
		/** Reload the overview after a cleanup. */
		onchange?: () => void;
	}

	let { overview, canClean, onchange }: Props = $props();

	let fresh = $state<DiskUsage | null>(null);
	let disk = $derived(
		fresh && (!overview?.disk || fresh.collected_at >= overview.disk.collected_at)
			? fresh
			: (overview?.disk ?? null)
	);
	let fs = $derived(overview?.current?.data_fs ?? null);
	let forecast = $derived(overview ? forecastSummary(overview.forecast) : null);
	let segments = $derived(disk && fs ? diskSegments(disk, fs.used, fs.total) : []);
	let now = $derived.by(() => {
		void disk;
		return Date.now() / 1000;
	});

	const refresh = useAction(() => refreshDiskUsage(), {
		onSuccess: (d) => (fresh = d)
	});

	let freed = $state('');
	let previewError = $state('');
	let checking = $state(false);

	function previewDetails(p: CleanupPreview): string[] {
		const details: string[] = [];
		if (p.build_cache_bytes > 0)
			details.push(
				`Unused build cache: ${formatBytes(p.build_cache_bytes)} (${p.build_cache_entries} entr${p.build_cache_entries === 1 ? 'y' : 'ies'}). The next build of each app takes a little longer.`
			);
		if (p.dangling_images.length > 0)
			details.push(
				`${p.dangling_images.length} dangling image${p.dangling_images.length === 1 ? '' : 's'}: ${formatBytes(p.dangling_bytes)} (untagged leftovers of old builds, used by no container: ${p.dangling_images.map((i) => i.id).join(', ')}).`
			);
		return details;
	}

	async function handleFreeUp() {
		freed = '';
		previewError = '';
		checking = true;
		let preview: CleanupPreview;
		try {
			preview = await getCleanupPreview();
		} catch (e: any) {
			previewError = e.message ?? "Couldn't check what can be removed.";
			return;
		} finally {
			checking = false;
		}
		if (preview.total_bytes <= 0) {
			freed = 'Nothing to clean up: there is no unused build cache and no dangling image.';
			return;
		}
		await confirmAction({
			title: `Free up to ${formatBytes(preview.total_bytes)}?`,
			description:
				"Only things Tond can rebuild are removed. Your apps' images (including rollback versions), containers, volumes and backups are never touched.",
			details: previewDetails(preview),
			confirmLabel: 'Free up space',
			pendingLabel: 'Freeing up space…',
			onconfirm: async () => {
				const result = await freeUpSpace();
				freed =
					result.total_bytes > 0
						? `Freed ${formatBytes(result.total_bytes)}: ${formatBytes(result.build_cache_bytes)} of build cache and ${result.images_deleted} image${result.images_deleted === 1 ? '' : 's'} (${formatBytes(result.image_bytes)}).`
						: 'Done. Docker reported nothing to remove (another cleanup may have run already).';
				fresh = null;
				onchange?.();
			}
		});
	}
</script>

<Card.Root>
	<Card.Header>
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div class="space-y-1.5">
				<Card.Title class="flex items-center gap-2">
					<HardDrive class="size-5 text-muted-foreground" aria-hidden="true" />
					Disk
				</Card.Title>
				<Card.Description>
					{#if fs}
						{formatBytes(fs.used)} of {formatBytes(fs.total)} used on {fs.mount} ({Math.round(
							(fs.used / Math.max(1, fs.total)) * 100
						)}%){fs.inodes_total > 0
							? ` · inodes ${((fs.inodes_used / fs.inodes_total) * 100).toFixed(1)}%`
							: ''}
					{:else}
						The filesystem that holds Tond's data
					{/if}
				</Card.Description>
			</div>
			<div class="flex flex-wrap gap-2">
				<Button
					size="sm"
					variant="outline"
					onclick={() => refresh.run()}
					disabled={refresh.pending}
				>
					<RefreshIcon class={cn('size-4', refresh.pending && 'animate-spin')} aria-hidden="true" />
					{refresh.pending ? 'Measuring…' : 'Measure again'}
				</Button>
				{#if canClean}
					<Button size="sm" onclick={handleFreeUp} disabled={checking}>
						<BroomIcon class="size-4" aria-hidden="true" />
						{checking ? 'Checking…' : 'Free up space'}
					</Button>
				{/if}
			</div>
		</div>
	</Card.Header>
	<Card.Content class="space-y-5">
		<InlineError
			message={refresh.error || previewError}
			ondismiss={() => ((previewError = ''), refresh.clear())}
		/>
		<SuccessAlert message={freed} ondismiss={() => (freed = '')} />

		{#if forecast}
			<div
				class={cn(
					'flex items-start gap-3 rounded-md border p-3',
					forecast.tone === 'danger' && 'border-destructive/40 bg-destructive/10',
					forecast.tone === 'warning' && 'border-warn/40 bg-warn/10',
					forecast.tone === 'success' && 'border-success/40 bg-success/10'
				)}
				data-testid="disk-forecast"
			>
				<TrendIcon
					class={cn(
						'mt-0.5 size-4 shrink-0',
						forecast.tone === 'danger' && 'text-destructive',
						forecast.tone === 'warning' && 'text-warn',
						forecast.tone === 'success' && 'text-success',
						forecast.tone === 'neutral' && 'text-muted-foreground'
					)}
					aria-hidden="true"
				/>
				<div class="text-sm">
					<p class="font-medium">{forecast.headline}</p>
					<p class="text-muted-foreground">{forecast.detail}</p>
				</div>
			</div>
		{/if}

		{#if !disk}
			<div class="space-y-2" aria-busy="true">
				<Skeleton class="h-4" />
				<p class="text-xs text-muted-foreground">
					The breakdown is measured a couple of minutes after start, then every 30 minutes.
				</p>
			</div>
		{:else}
			{#if disk.error}
				<InlineError message={disk.error} />
			{/if}
			{#if fs && segments.length > 0}
				<div
					class="flex h-4 w-full overflow-hidden rounded-sm bg-muted"
					role="img"
					aria-label={segments.map((s) => `${s.label} ${formatBytes(s.bytes)}`).join(', ')}
				>
					{#each segments as seg (seg.key)}
						<span
							class="h-full"
							style:width="{(seg.bytes / Math.max(1, fs.total)) * 100}%"
							style:background-color={seg.color}
							title="{seg.label}: {formatBytes(seg.bytes)}"
						></span>
					{/each}
				</div>
				<ul class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-2 lg:grid-cols-3">
					{#each segments as seg (seg.key)}
						<li class="flex min-w-0 items-start gap-2">
							<span
								class="mt-1 size-2.5 shrink-0 rounded-sm border border-border"
								style:background-color={seg.color}
							></span>
							<span class="min-w-0">
								<span class="font-medium tabular-nums">{formatBytes(seg.bytes)}</span>
								<span>{seg.label}</span>
								{#if seg.hint}
									<span class="block text-xs text-muted-foreground">{seg.hint}</span>
								{/if}
							</span>
						</li>
					{/each}
				</ul>
			{/if}
			{#if !disk.docker_on_data_fs}
				<p class="text-sm text-muted-foreground">
					Docker keeps its data on another filesystem ({disk.docker_root ?? 'unknown'}): images {formatBytes(
						disk.images.bytes
					)}, build cache {formatBytes(disk.build_cache.bytes)}, containers {formatBytes(
						disk.containers.bytes
					)}.
				</p>
			{/if}
			<p class="text-xs text-muted-foreground">
				Measured {relativeUnix(disk.collected_at, now)} (took {(disk.took_ms / 1000).toFixed(1)} s) ·
				refreshed every 30 minutes. Reclaimable by Docker: {formatBytes(
					disk.build_cache.reclaimable
				)} of build cache;
				{formatBytes(disk.images.reclaimable)} in images no container runs (mostly rollback versions,
				which Tond keeps on purpose).
			</p>
		{/if}
	</Card.Content>
</Card.Root>
