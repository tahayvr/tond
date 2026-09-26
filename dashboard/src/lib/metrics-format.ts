// Formatting and chart geometry helpers for the resource metrics panel.

import type { MetricEvent, MetricsRange } from '$lib/api';

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
export function formatTime(t: number, range: MetricsRange, withDate = false): string {
	const d = new Date(t * 1000);
	if (range === '7d' || range === '30d') {
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

export const EVENT_LABELS: Record<MetricEvent['kind'], string> = {
	restart: 'Restarted',
	crash: 'Crashed',
	oom: 'Out of memory'
};

/** How often to refresh a range (seconds of data per new bucket). */
export function refreshInterval(range: MetricsRange): number {
	switch (range) {
		case '1h':
			return 30_000;
		case '24h':
			return 60_000;
		case '7d':
			return 5 * 60_000;
		case '30d':
			return 10 * 60_000;
	}
}
