<script lang="ts" module>
	/** Height of one log row in px (Tailwind h-5). Rows never wrap. */
	export const ROW_HEIGHT = 20;
</script>

<script lang="ts">
	// One log viewer for runtime, deploy, database and cron output.
	//
	// - ANSI colours (readable in both themes: --ansi-* variables)
	// - search with highlight, match count and next/previous (Enter / Shift+Enter)
	// - follow: sticks to the newest line until the user scrolls up, then shows
	//   a "new output" button instead of jumping
	// - timestamps toggle, download as text
	// - virtualized: only the visible rows (plus overscan) are in the DOM, so
	//   thousands of lines stay cheap
	import { tick, type Snippet } from 'svelte';
	import type { LogLine } from '$lib/api';
	import { segmentStyle } from '$lib/ansi';
	import {
		findMatches,
		formatTime,
		highlightSegments,
		logFileName,
		prepareLine,
		toDownloadText,
		visibleRange,
		type PreparedLine
	} from '$lib/log-view';
	import { cn } from '$lib/utils';
	import { Button } from '$lib/components/ui/button/index.js';
	import SearchIcon from '~icons/ri/search-line';
	import UpIcon from '~icons/ri/arrow-up-s-line';
	import DownIcon from '~icons/ri/arrow-down-s-line';
	import DownloadIcon from '~icons/ri/download-2-line';
	import TimeIcon from '~icons/ri/time-line';
	import PauseIcon from '~icons/ri/pause-line';
	import PlayIcon from '~icons/ri/play-line';
	import ArrowDownIcon from '~icons/ri/arrow-down-line';
	import CloseIcon from '~icons/ri/close-line';

	interface Props {
		lines: LogLine[];
		/** Output is still arriving (shows a Live marker). */
		live?: boolean;
		/** Base of the download file name, e.g. "web-deploy". */
		downloadName?: string;
		/** Shown when there are no lines. */
		emptyText?: string;
		/** Viewport height (Tailwind class). */
		heightClass?: string;
		/** Show the app/database name carried by multi-source streams. */
		showPrefix?: boolean;
		/** Timestamps shown initially. */
		timestamps?: boolean;
		/** Extra toolbar controls (connect, clear, …). */
		actions?: Snippet;
		/** Accessible name of the log region. */
		label?: string;
		class?: string;
	}

	let {
		lines,
		live = false,
		downloadName = 'logs',
		emptyText = 'No output yet.',
		heightClass = 'h-[28rem]',
		showPrefix = true,
		timestamps = true,
		actions,
		label = 'Log output',
		class: className
	}: Props = $props();

	// Prepared lines are cached per source object: a buffer flush copies the
	// array but keeps the line objects, so each line is parsed once.
	const cache = new WeakMap<LogLine, PreparedLine>();
	let prepared = $derived(
		lines.map((line, i) => {
			let p = cache.get(line);
			if (!p) {
				p = prepareLine(line, i, showPrefix);
				cache.set(line, p);
			}
			return p;
		})
	);

	let query = $state('');
	let matches = $derived(findMatches(prepared, query));
	let current = $state(0);
	/** Follows the `timestamps` prop until the user toggles it. */
	let showTs = $derived(timestamps);
	let follow = $state(true);
	let pausedLast = $state<LogLine | null>(null);

	let viewport = $state<HTMLElement | null>(null);
	let scrollTop = $state(0);
	let viewportHeight = $state(0);

	let range = $derived(visibleRange(scrollTop, viewportHeight || 480, ROW_HEIGHT, prepared.length));
	let rows = $derived(prepared.slice(range.start, range.end));
	let currentLine = $derived(matches.length ? matches[Math.min(current, matches.length - 1)] : -1);
	let hasNew = $derived(
		!follow && lines.length > 0 && pausedLast !== null && lines[lines.length - 1] !== pausedLast
	);

	function scrollToBottom() {
		if (!viewport) return;
		viewport.scrollTop = viewport.scrollHeight;
		scrollTop = viewport.scrollTop;
	}

	// Follow the newest line while following.
	$effect(() => {
		void prepared.length;
		void lines[lines.length - 1];
		if (follow) tick().then(scrollToBottom);
	});

	// Track the viewport size.
	$effect(() => {
		const el = viewport;
		if (!el) return;
		viewportHeight = el.clientHeight;
		if (typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver(() => (viewportHeight = el.clientHeight));
		ro.observe(el);
		return () => ro.disconnect();
	});

	function handleScroll() {
		if (!viewport) return;
		scrollTop = viewport.scrollTop;
		const fromBottom = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
		const atBottom = fromBottom <= ROW_HEIGHT;
		if (follow && !atBottom) pause();
		else if (!follow && atBottom) resume();
	}

	function pause() {
		follow = false;
		pausedLast = lines[lines.length - 1] ?? null;
	}

	function resume() {
		follow = true;
		pausedLast = null;
		tick().then(scrollToBottom);
	}

	function toggleFollow() {
		if (follow) pause();
		else resume();
	}

	function revealLine(index: number) {
		if (!viewport || index < 0) return;
		pause();
		const target = index * ROW_HEIGHT - viewport.clientHeight / 2 + ROW_HEIGHT;
		viewport.scrollTop = Math.max(0, target);
		scrollTop = viewport.scrollTop;
	}

	function step(delta: number) {
		if (matches.length === 0) return;
		current = (current + delta + matches.length) % matches.length;
		revealLine(matches[current]);
	}

	function handleSearchInput(e: Event) {
		query = (e.target as HTMLInputElement).value;
		current = 0;
		if (matches.length) revealLine(matches[0]);
	}

	function handleSearchKey(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			step(e.shiftKey ? -1 : 1);
		} else if (e.key === 'Escape') {
			query = '';
		}
	}

	function handleDownload() {
		const blob = new Blob([toDownloadText(prepared, showTs)], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = logFileName(downloadName);
		document.body.appendChild(a);
		a.click();
		a.remove();
		setTimeout(() => URL.revokeObjectURL(url), 0);
	}

	function levelClass(level: string | undefined): string {
		if (level === 'error') return 'text-[var(--ansi-1)]';
		if (level === 'warn') return 'text-[var(--ansi-3)]';
		return '';
	}
</script>

<div class={cn('overflow-hidden rounded-md border bg-log-bg text-log-fg', className)}>
	<div class="flex flex-wrap items-center gap-2 border-b bg-background/60 px-2 py-1.5">
		<div class="flex min-w-0 flex-1 items-center gap-1 sm:max-w-xs">
			<div class="relative min-w-0 flex-1">
				<SearchIcon
					class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground"
				/>
				<input
					type="search"
					value={query}
					oninput={handleSearchInput}
					onkeydown={handleSearchKey}
					placeholder="Search logs"
					aria-label="Search logs"
					class="h-7 w-full rounded-sm border bg-background pr-2 pl-7 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
				/>
			</div>
			{#if query.trim()}
				<span class="shrink-0 text-[11px] text-muted-foreground tabular-nums" aria-live="polite">
					{matches.length
						? `${Math.min(current, matches.length - 1) + 1}/${matches.length}`
						: '0/0'}
				</span>
				<Button
					variant="ghost"
					size="icon-sm"
					class="size-7"
					onclick={() => step(-1)}
					disabled={!matches.length}
					aria-label="Previous match"><UpIcon /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					class="size-7"
					onclick={() => step(1)}
					disabled={!matches.length}
					aria-label="Next match"><DownIcon /></Button
				>
				<Button
					variant="ghost"
					size="icon-sm"
					class="size-7"
					onclick={() => (query = '')}
					aria-label="Clear search"><CloseIcon /></Button
				>
			{/if}
		</div>
		<div class="flex items-center gap-1">
			{#if live}
				<span class="mr-1 flex items-center gap-1.5 text-[11px] text-muted-foreground">
					<span class="inline-block size-1.5 animate-pulse rounded-full bg-success"></span>
					Live
				</span>
			{/if}
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 text-[11px]"
				onclick={toggleFollow}
				aria-pressed={follow}
				title={follow ? 'Pause auto-scroll' : 'Follow new output'}
			>
				{#if follow}<PauseIcon class="size-3.5" /> Following{:else}<PlayIcon class="size-3.5" /> Follow{/if}
			</Button>
			<Button
				variant="ghost"
				size="sm"
				class={cn('h-7 px-2 text-[11px]', !showTs && 'text-muted-foreground')}
				onclick={() => (showTs = !showTs)}
				aria-pressed={showTs}
				title="Show timestamps"
			>
				<TimeIcon class="size-3.5" /> Time
			</Button>
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 text-[11px]"
				onclick={handleDownload}
				disabled={prepared.length === 0}
				title="Download as text"
			>
				<DownloadIcon class="size-3.5" /> Save
			</Button>
			{@render actions?.()}
		</div>
	</div>

	<div class="relative">
		<!-- A scrollable region must be focusable so keyboard users can scroll it. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			bind:this={viewport}
			onscroll={handleScroll}
			class={cn('overflow-auto font-mono text-xs', heightClass)}
			role="log"
			aria-label={label}
			tabindex="0"
		>
			{#if prepared.length === 0}
				<div class="flex h-full items-center justify-center px-4 text-center text-muted-foreground">
					{emptyText}
				</div>
			{:else}
				<div class="relative w-max min-w-full" style="height: {prepared.length * ROW_HEIGHT}px">
					<div style="transform: translateY({range.start * ROW_HEIGHT}px)">
						{#each rows as line, i (range.start + i)}
							{@const index = range.start + i}
							<div
								class={cn(
									'flex h-5 items-center px-3 leading-5 whitespace-pre',
									levelClass(line.level),
									index === currentLine && 'bg-[var(--ansi-3)]/15'
								)}
								data-line={index}
							>
								{#if showTs}
									<span
										class="mr-3 inline-block w-[4.5rem] shrink-0 text-muted-foreground tabular-nums select-none"
										>{formatTime(line.ts)}</span
									>
								{/if}
								{#if line.prefix}
									<span class="mr-2 shrink-0 text-[var(--ansi-4)]">{line.prefix}</span>
								{/if}
								<span
									>{#each query.trim() ? highlightSegments(line.segments, query) : line.segments as seg, j (j)}<span
											style={segmentStyle(seg)}
											class={'match' in seg && seg.match
												? 'rounded-[2px] bg-[var(--ansi-3)]/35 text-inherit'
												: undefined}>{seg.text}</span
										>{/each}</span
								>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</div>
		{#if hasNew}
			<button
				type="button"
				onclick={resume}
				class="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full border bg-background px-3 py-1 text-xs shadow-md hover:bg-accent"
			>
				<ArrowDownIcon class="size-3.5" /> New output below
			</button>
		{/if}
	</div>
</div>
