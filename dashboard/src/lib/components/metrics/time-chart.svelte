<script lang="ts">
	// Small SVG time-series chart: one y-axis, gaps break the line, optional
	// limit line, restart / crash / OOM markers, crosshair tooltip (pointer and
	// arrow keys). Series values share one unit (never two y-scales).
	import type { MetricEvent, MetricsRange } from '$lib/api';
	import { EVENT_LABELS, formatTime, nearestIndex, segments, timeTicks } from '$lib/metrics-format';

	export interface ChartSeries {
		key: string;
		label: string;
		values: number[];
		/** CSS color (a theme variable). */
		color: string;
		dashed?: boolean;
		/** Fill the area under the line (single-series charts). */
		area?: boolean;
	}

	interface Props {
		/** Accessible name, e.g. "CPU usage". */
		label: string;
		times: number[];
		series: ChartSeries[];
		from: number;
		to: number;
		step: number;
		range: MetricsRange;
		yMax: number;
		yFormat: (v: number) => string;
		limit?: { value: number; label: string } | null;
		events?: MetricEvent[];
		height?: number;
	}

	let {
		label,
		times,
		series,
		from,
		to,
		step,
		range,
		yMax,
		yFormat,
		limit = null,
		events = [],
		height = 180
	}: Props = $props();

	const PAD = { top: 10, right: 12, bottom: 22, left: 64 };

	let width = $state(600);
	let hover = $state<number | null>(null);

	let plotW = $derived(Math.max(10, width - PAD.left - PAD.right));
	let plotH = $derived(height - PAD.top - PAD.bottom);
	let span = $derived(Math.max(1, to - from));

	function x(t: number): number {
		return PAD.left + ((t - from) / span) * plotW;
	}
	function y(v: number): number {
		return PAD.top + plotH - (Math.min(v, yMax) / yMax) * plotH;
	}

	let runs = $derived(segments(times, step));

	function linePath(values: number[]): string {
		return runs
			.map((run) =>
				run
					.map(
						(i, j) => `${j === 0 ? 'M' : 'L'}${x(times[i]).toFixed(1)},${y(values[i]).toFixed(1)}`
					)
					.join('')
			)
			.join('');
	}

	function areaPath(values: number[]): string {
		const base = (PAD.top + plotH).toFixed(1);
		return runs
			.map((run) => {
				// A single-point run is drawn as a bucket-wide bar so it stays visible.
				const first = x(times[run[0]]).toFixed(1);
				const last = x(times[run[run.length - 1]] + (run.length === 1 ? step : 0)).toFixed(1);
				const top = run
					.map((i) => `L${x(times[i]).toFixed(1)},${y(values[i]).toFixed(1)}`)
					.join('');
				const tail = run.length === 1 ? `L${last},${y(values[run[0]]).toFixed(1)}` : '';
				return `M${first},${base}${top}${tail}L${last},${base}Z`;
			})
			.join('');
	}

	let gridValues = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => f * yMax));
	let ticks = $derived(timeTicks(from, to, width < 480 ? 3 : 5));

	let hoverEvents = $derived.by(() => {
		if (hover === null) return [];
		const t = times[hover];
		return events.filter((e) => e.t >= t - step / 2 && e.t < t + step * 1.5);
	});

	function handlePointer(e: PointerEvent) {
		const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
		const px = e.clientX - rect.left;
		const t = from + ((px - PAD.left) / plotW) * span;
		const i = nearestIndex(times, t);
		hover = i >= 0 ? i : null;
	}

	function handleKey(e: KeyboardEvent) {
		if (times.length === 0) return;
		if (e.key === 'ArrowLeft') {
			hover = Math.max(0, (hover ?? times.length) - 1);
			e.preventDefault();
		} else if (e.key === 'ArrowRight') {
			hover = Math.min(times.length - 1, (hover ?? -1) + 1);
			e.preventDefault();
		} else if (e.key === 'Escape') {
			hover = null;
		}
	}

	function eventColor(kind: MetricEvent['kind']): string {
		return kind === 'restart' ? 'var(--warn)' : 'var(--destructive)';
	}

	let tooltipLeft = $derived(hover === null ? 0 : x(times[hover]));

	// Read out by screen readers as the arrow keys move the crosshair.
	let valueText = $derived.by(() => {
		const i = hover ?? times.length - 1;
		if (i < 0) return 'No data';
		const values = series.map((s) => `${s.label} ${yFormat(s.values[i])}`).join(', ');
		return `${formatTime(times[i], range, true)}: ${values}`;
	});
</script>

<div class="relative w-full" bind:clientWidth={width}>
	<div
		role="slider"
		aria-label="{label} chart. Use the left and right arrow keys to read values."
		aria-valuemin={0}
		aria-valuemax={Math.max(0, times.length - 1)}
		aria-valuenow={hover ?? times.length - 1}
		aria-valuetext={valueText}
		tabindex="0"
		class="touch-pan-y rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
		onpointermove={handlePointer}
		onpointerdown={handlePointer}
		onpointerleave={() => (hover = null)}
		onkeydown={handleKey}
		onblur={() => (hover = null)}
	>
		<svg aria-hidden="true" {width} {height} class="block overflow-visible">
			<!-- grid + y labels -->
			{#each gridValues as v (v)}
				<line
					x1={PAD.left}
					x2={PAD.left + plotW}
					y1={y(v)}
					y2={y(v)}
					class="stroke-border"
					stroke-width="1"
					stroke-dasharray={v === 0 ? undefined : '2 4'}
				/>
				<text
					x={PAD.left - 8}
					y={y(v)}
					dy="0.32em"
					text-anchor="end"
					class="fill-muted-foreground text-[10px] tabular-nums">{yFormat(v)}</text
				>
			{/each}

			<!-- x labels -->
			{#each ticks as t, i (t)}
				<text
					x={x(t)}
					y={height - 6}
					text-anchor={i === 0 ? 'start' : i === ticks.length - 1 ? 'end' : 'middle'}
					class="fill-muted-foreground text-[10px] tabular-nums">{formatTime(t, range)}</text
				>
			{/each}

			<!-- limit -->
			{#if limit && limit.value <= yMax}
				<line
					x1={PAD.left}
					x2={PAD.left + plotW}
					y1={y(limit.value)}
					y2={y(limit.value)}
					class="stroke-muted-foreground"
					stroke-width="1"
					stroke-dasharray="6 4"
				/>
				<text
					x={PAD.left + 4}
					y={y(limit.value) - 4}
					text-anchor="start"
					class="fill-muted-foreground text-[10px]">{limit.label}</text
				>
			{/if}

			<!-- event markers -->
			{#each events as ev (`${ev.t}-${ev.kind}`)}
				{#if ev.t >= from && ev.t <= to}
					<line
						x1={x(ev.t)}
						x2={x(ev.t)}
						y1={PAD.top}
						y2={PAD.top + plotH}
						stroke={eventColor(ev.kind)}
						stroke-width="1.5"
						stroke-dasharray="3 3"
					/>
					<circle cx={x(ev.t)} cy={PAD.top} r="3.5" fill={eventColor(ev.kind)} />
				{/if}
			{/each}

			<!-- series -->
			{#each series as s (s.key)}
				{#if s.area}
					<path d={areaPath(s.values)} fill={s.color} fill-opacity="0.12" />
				{/if}
				<path
					d={linePath(s.values)}
					fill="none"
					stroke={s.color}
					stroke-width="2"
					stroke-linejoin="round"
					stroke-linecap="round"
					stroke-dasharray={s.dashed ? '5 3' : undefined}
				/>
			{/each}

			<!-- crosshair -->
			{#if hover !== null}
				<line
					x1={x(times[hover])}
					x2={x(times[hover])}
					y1={PAD.top}
					y2={PAD.top + plotH}
					class="stroke-foreground/40"
					stroke-width="1"
				/>
				{#each series as s (s.key)}
					<circle
						cx={x(times[hover])}
						cy={y(s.values[hover])}
						r="4"
						fill={s.color}
						class="stroke-card"
						stroke-width="2"
					/>
				{/each}
			{/if}
		</svg>
	</div>

	{#if hover !== null}
		<div
			class="pointer-events-none absolute top-0 z-10 min-w-36 rounded-md border bg-popover px-3 py-2 text-xs shadow-md"
			style:left="{tooltipLeft}px"
			style:transform={tooltipLeft > width / 2
				? 'translateX(calc(-100% - 12px))'
				: 'translateX(12px)'}
		>
			<p class="mb-1 text-muted-foreground">{formatTime(times[hover], range, true)}</p>
			{#each series as s (s.key)}
				<div class="flex items-center gap-2">
					<svg width="14" height="4" aria-hidden="true" class="shrink-0">
						<line
							x1="0"
							x2="14"
							y1="2"
							y2="2"
							stroke={s.color}
							stroke-width="2"
							stroke-dasharray={s.dashed ? '4 2' : undefined}
						/>
					</svg>
					<span class="font-semibold text-foreground tabular-nums">{yFormat(s.values[hover])}</span>
					<span class="text-muted-foreground">{s.label}</span>
				</div>
			{/each}
			{#each hoverEvents as ev (`${ev.t}-${ev.kind}`)}
				<p class="mt-1 text-foreground">
					<span class="font-semibold">{EVENT_LABELS[ev.kind]}</span>
					<span class="text-muted-foreground">
						{formatTime(ev.t, '1h')}{ev.detail ? ` · ${ev.detail}` : ''}</span
					>
				</p>
			{/each}
		</div>
	{/if}
</div>
