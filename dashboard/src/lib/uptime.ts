// Formatting for uptime checks (GET /api/apps/{id}/uptime, /api/uptime).

import type { UptimeBar, UptimeCause, UptimeStatus } from '$lib/api';

/** 99.95 → "99.95%", 100 → "100%", 97.3 → "97.3%", null → "—". */
export function formatUptime(percent: number | null | undefined): string {
	if (percent === null || percent === undefined || !Number.isFinite(percent)) return '—';
	if (percent >= 100) return '100%';
	if (percent >= 99) {
		// Never round 99.996 up to a misleading "100.00%".
		const floored = Math.floor(percent * 100) / 100;
		return `${floored.toFixed(2)}%`;
	}
	return `${(Math.floor(percent * 10) / 10).toFixed(1)}%`;
}

export type BarTone = 'none' | 'up' | 'partial' | 'down';

/** Colour class of one status bar: all up, mostly up, mostly down, or no checks. */
export function barTone(bar: Pick<UptimeBar, 'probes' | 'up'>): BarTone {
	if (bar.probes <= 0) return 'none';
	const ratio = bar.up / bar.probes;
	if (ratio >= 1) return 'up';
	if (ratio >= 0.9) return 'partial';
	return 'down';
}

export const BAR_CLASSES: Record<BarTone, string> = {
	none: 'bg-border',
	up: 'bg-success',
	partial: 'bg-warn',
	down: 'bg-destructive'
};

const CAUSE_LABELS: Record<UptimeCause, string> = {
	dns: 'DNS',
	refused: 'Connection refused',
	connect: 'Connection failed',
	timeout: 'Timeout',
	tls: 'TLS certificate',
	http_5xx: 'Server error (5xx)',
	http_status: 'Unexpected status'
};

export function causeLabel(cause: UptimeCause | null | undefined): string {
	return cause ? (CAUSE_LABELS[cause] ?? cause) : '';
}

/** 45 → "45 s", 180 → "3 min", 4380 → "1 h 13 min", 190000 → "2 d 5 h". */
export function formatDuration(secs: number): string {
	const s = Math.max(0, Math.round(secs));
	if (s < 60) return `${s} s`;
	const min = Math.floor(s / 60);
	if (min < 60) return `${min} min`;
	const h = Math.floor(min / 60);
	if (h < 24) return min % 60 ? `${h} h ${min % 60} min` : `${h} h`;
	const d = Math.floor(h / 24);
	return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** Tooltip text for a bar: "Sep 25, 10:30 · 100% up (30 checks)". */
export function barTitle(bar: UptimeBar, width: 'half-hour' | 'day'): string {
	const d = new Date(bar.t * 1000);
	const when =
		width === 'day'
			? d.toLocaleDateString([], { month: 'short', day: 'numeric' })
			: `${d.toLocaleDateString([], { month: 'short', day: 'numeric' })}, ${d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`;
	if (bar.probes <= 0) return `${when} · no checks`;
	const pct = formatUptime((bar.up / bar.probes) * 100);
	const failed = bar.probes - bar.up;
	return `${when} · ${pct} up (${bar.probes} check${bar.probes === 1 ? '' : 's'}${failed > 0 ? `, ${failed} failed` : ''})`;
}

/** Short summary of a strip for screen readers. */
export function barsSummary(bars: UptimeBar[]): string {
	const checked = bars.filter((b) => b.probes > 0);
	if (checked.length === 0) return 'No checks in this period';
	const bad = checked.filter((b) => barTone(b) !== 'up').length;
	return bad === 0
		? `Up in every period (${checked.length} with checks)`
		: `${bad} of ${checked.length} periods had failed checks`;
}

/** Status line for a check. */
export function statusText(status: UptimeStatus): string {
	switch (status) {
		case 'up':
			return 'Up';
		case 'down':
			return 'Down';
		case 'paused':
			return 'Paused';
		default:
			return 'Waiting for the first check';
	}
}

/** Unix seconds relative to `now` (seconds): "just now", "3 min ago", "in 40 s". */
export function relativeUnix(t: number | null | undefined, now: number): string {
	if (t === null || t === undefined) return 'never';
	const diff = t - now;
	const abs = Math.abs(diff);
	if (abs < 5) return 'just now';
	let text: string;
	if (abs < 60) text = `${Math.round(abs)} s`;
	else if (abs < 3600) text = `${Math.round(abs / 60)} min`;
	else if (abs < 86_400) text = `${Math.round(abs / 3600)} h`;
	else text = `${Math.round(abs / 86_400)} d`;
	return diff < 0 ? `${text} ago` : `in ${text}`;
}

/** Choices for the check interval (seconds). */
export const INTERVAL_OPTIONS: { value: number; label: string }[] = [
	{ value: 60, label: 'Every minute' },
	{ value: 120, label: 'Every 2 minutes' },
	{ value: 300, label: 'Every 5 minutes' },
	{ value: 600, label: 'Every 10 minutes' },
	{ value: 900, label: 'Every 15 minutes' },
	{ value: 1800, label: 'Every 30 minutes' },
	{ value: 3600, label: 'Every hour' }
];

/** Validate the check form; returns an error message or ''. */
export function validateCheckForm(form: {
	path: string;
	expectedStatus: string;
	timeoutSecs: number;
}): string {
	const path = form.path.trim();
	if (!path.startsWith('/')) return 'The path must start with / (for example /health).';
	if (/\s/.test(path)) return "The path can't contain spaces.";
	const status = form.expectedStatus.trim();
	if (status !== '') {
		const n = Number(status);
		if (!Number.isInteger(n) || n < 100 || n > 599)
			return 'The expected status must be an HTTP status code between 100 and 599, or empty for any 2xx/3xx.';
	}
	if (!Number.isInteger(form.timeoutSecs) || form.timeoutSecs < 1 || form.timeoutSecs > 30)
		return 'The timeout must be a whole number of seconds between 1 and 30.';
	return '';
}
