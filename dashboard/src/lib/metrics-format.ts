// Formatting and chart geometry helpers for the resource and request
// metrics panels.

import type { MetricEvent, MetricsRange, RequestMarkerKind, RequestRange } from '$lib/api';

/** Any range a chart can show (resource and request metrics). */
export type ChartRange = MetricsRange | RequestRange;

/** A marker drawn on a chart: container events, deploys and rollbacks. */
export interface ChartMarker {
	t: number;
	kind: RequestMarkerKind;
	detail: string | null;
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

/** 1536 → "1.5 KB" (binary units, like the rest of the dashboard). */
export function formatBytes(bytes: number, digits = 1): string {
	if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
	const i = Math.min(UNITS.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
	const v = bytes / 1024 ** i;
	return `${i === 0 ? Math.round(v) : v.toFixed(v >= 100 ? 0 : digits)} ${UNITS[i]}`;
}

/** Bytes per second → "1.5 KB/s". */
export function formatRate(bytesPerSecond: number): string {
	return `${formatBytes(bytesPerSecond)}/s`;
}

/** CPU percent of one core → "12.5%". */
export function formatCpu(percent: number): string {
	if (percent >= 100) return `${Math.round(percent)}%`;
	return `${percent.toFixed(1)}%`;
}

/** Axis / tooltip time label for a unix-seconds timestamp. */
export function formatTime(t: number, range: ChartRange, withDate = false): string {
	const d = new Date(t * 1000);
	if (range === '7d' || range === '30d' || range === '90d') {
		const date = d.toLocaleDateString([], { month: 'short', day: 'numeric' });
		return withDate
			? `${date} ${d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`
			: date;
	}
	const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	return withDate
		? `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })} ${time}`
		: time;
}

/** A round axis maximum ≥ `max` (1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8 × 10ⁿ). */
export function niceMax(max: number): number {
	if (!Number.isFinite(max) || max <= 0) return 1;
	const exp = 10 ** Math.floor(Math.log10(max));
	for (const m of [1, 1.2, 1.5, 2, 2.5, 3, 4, 5, 6, 8, 10]) {
		if (m * exp >= max) return m * exp;
	}
	return 10 * exp;
}

/** A round byte axis maximum (powers of two steps of KB/MB/GB). */
export function niceBytesMax(max: number): number {
	if (!Number.isFinite(max) || max <= 0) return 1024;
	const unit = 1024 ** Math.max(0, Math.floor(Math.log(max) / Math.log(1024)));
	return niceMax(max / unit) * unit;
}

/**
 * Split series indexes into contiguous runs: a gap longer than two buckets
 * (the thing was stopped, or the server was down) breaks the line.
 */
export function segments(times: number[], step: number): number[][] {
	const runs: number[][] = [];
	let run: number[] = [];
	for (let i = 0; i < times.length; i++) {
		if (run.length > 0 && times[i] - times[i - 1] > step * 2) {
			runs.push(run);
			run = [];
		}
		run.push(i);
	}
	if (run.length > 0) runs.push(run);
	return runs;
}

/** Index of the time closest to `t` (times sorted ascending), or -1. */
export function nearestIndex(times: number[], t: number): number {
	if (times.length === 0) return -1;
	let lo = 0;
	let hi = times.length - 1;
	while (hi - lo > 1) {
		const mid = (lo + hi) >> 1;
		if (times[mid] <= t) lo = mid;
		else hi = mid;
	}
	return Math.abs(times[hi] - t) < Math.abs(times[lo] - t) ? hi : lo;
}

/** `count` evenly spaced tick times between from and to. */
export function timeTicks(from: number, to: number, count = 5): number[] {
	if (to <= from) return [from];
	const span = to - from;
	return Array.from({ length: count }, (_, i) => Math.round(from + (span * i) / (count - 1)));
}

export interface EventCounts {
	restarts: number;
	crashes: number;
	ooms: number;
}

export function countEvents(events: MetricEvent[]): EventCounts {
	return {
		restarts: events.filter((e) => e.kind === 'restart').length,
		crashes: events.filter((e) => e.kind === 'crash').length,
		ooms: events.filter((e) => e.kind === 'oom').length
	};
}

export const EVENT_LABELS: Record<RequestMarkerKind, string> = {
	restart: 'Restarted',
	crash: 'Crashed',
	oom: 'Out of memory',
	deploy: 'Deployed',
	rollback: 'Rolled back'
};

/** Colour of a chart marker (theme variables). */
export function markerColor(kind: RequestMarkerKind): string {
	switch (kind) {
		case 'deploy':
		case 'rollback':
			return 'var(--info)';
		case 'restart':
			return 'var(--warn)';
		default:
			return 'var(--destructive)';
	}
}

/** How often to refresh a range (seconds of data per new bucket). */
export function refreshInterval(range: ChartRange): number {
	switch (range) {
		case '1h':
			return 30_000;
		case '6h':
		case '24h':
			return 60_000;
		case '7d':
			return 5 * 60_000;
		case '30d':
		case '90d':
			return 10 * 60_000;
	}
}

/** Requests per second → "0.25/s", "12/s", "1.2k/s". */
export function formatRps(rps: number): string {
	if (!Number.isFinite(rps) || rps <= 0) return '0/s';
	if (rps >= 1000) return `${(rps / 1000).toFixed(rps >= 10_000 ? 0 : 1)}k/s`;
	if (rps >= 10) return `${Math.round(rps)}/s`;
	if (rps >= 1) return `${rps.toFixed(1)}/s`;
	return `${rps.toFixed(2)}/s`;
}

/** Milliseconds → "4 ms", "12.5 ms", "1.2 s"; "—" for null. */
export function formatMs(ms: number | null | undefined): string {
	if (ms === null || ms === undefined || !Number.isFinite(ms)) return '—';
	if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10_000 ? 0 : 1)} s`;
	if (ms >= 100 || Number.isInteger(ms)) return `${Math.round(ms)} ms`;
	return `${ms.toFixed(1)} ms`;
}

/** Percent → "0%", "0.4%", "12%"; "—" for null. */
export function formatPercent(pct: number | null | undefined): string {
	if (pct === null || pct === undefined || !Number.isFinite(pct)) return '—';
	if (pct === 0) return '0%';
	if (pct < 0.1) return '<0.1%';
	if (pct < 10) return `${pct.toFixed(1)}%`;
	return `${Math.round(pct)}%`;
}

/** 1234567 → "1.2M", 5300 → "5.3k", 42 → "42". */
export function formatCount(n: number): string {
	if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
	if (n >= 1000) return `${(n / 1000).toFixed(n >= 100_000 ? 0 : 1)}k`;
	return String(Math.round(n));
}

/** Running totals of stacked series: result[k][i] = Σ series[0..=k][i]. */
export function stack(series: number[][]): number[][] {
	const out: number[][] = [];
	series.forEach((values, k) => {
		out.push(values.map((v, i) => (k === 0 ? 0 : out[k - 1][i]) + (v || 0)));
	});
	return out;
}

/** Largest non-null value (0 when there is none). */
export function maxOf(...lists: (number | null)[][]): number {
	let max = 0;
	for (const list of lists) for (const v of list) if (v !== null && v > max) max = v;
	return max;
}

/**
 * SVG path of a sparkline in a `width` × `height` box (values scaled to
 * `max`; nulls break the line). Empty string when nothing can be drawn.
 */
export function sparkPath(
	values: (number | null)[],
	width: number,
	height: number,
	max = maxOf(values)
): string {
	if (values.length === 0) return '';
	const top = max > 0 ? max : 1;
	const dx = values.length > 1 ? width / (values.length - 1) : 0;
	let d = '';
	let pen = false;
	values.forEach((v, i) => {
		if (v === null || !Number.isFinite(v)) {
			pen = false;
			return;
		}
		const x = (values.length > 1 ? i * dx : width / 2).toFixed(1);
		const y = (height - (Math.min(v, top) / top) * (height - 2) - 1).toFixed(1);
		d += `${pen ? 'L' : 'M'}${x},${y}`;
		pen = true;
	});
	return d;
}
