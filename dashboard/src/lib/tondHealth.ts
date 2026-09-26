// Formatting for the "Tond health" panel (GET /api/system/self).

import type { LoopStats } from '$lib/api';

export function formatBytes(bytes: number | null | undefined): string {
	if (bytes === null || bytes === undefined) return '–';
	if (bytes < 1024) return `${bytes} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
	if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatPercent(value: number | null | undefined): string {
	if (value === null || value === undefined) return '–';
	if (value > 0 && value < 0.1) return '<0.1%';
	return `${value.toFixed(1)}%`;
}

/** "850 ms", "4.2 s", "3 min" */
export function formatMs(ms: number | null | undefined): string {
	if (ms === null || ms === undefined) return '–';
	if (ms < 1000) return `${Math.round(ms)} ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
	return `${Math.round(ms / 60_000)} min`;
}

/** "45 s", "12 min", "3 h 5 min", "2 d 4 h" */
export function formatUptime(secs: number): string {
	if (secs < 60) return `${secs} s`;
	const min = Math.floor(secs / 60);
	if (min < 60) return `${min} min`;
	const h = Math.floor(min / 60);
	if (h < 24) return min % 60 ? `${h} h ${min % 60} min` : `${h} h`;
	const d = Math.floor(h / 24);
	return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** "just now", "3 min ago", "in 58 min", "in 2 h" relative to `now`. */
export function relativeTime(iso: string | null, now: number = Date.now()): string {
	if (!iso) return 'never';
	const diff = new Date(iso).getTime() - now;
	const abs = Math.abs(diff);
	if (abs < 5_000) return 'just now';
	let text: string;
	if (abs < 60_000) text = `${Math.round(abs / 1000)} s`;
	else if (abs < 3_600_000) text = `${Math.round(abs / 60_000)} min`;
	else if (abs < 86_400_000) text = `${Math.round(abs / 3_600_000)} h`;
	else text = `${Math.round(abs / 86_400_000)} d`;
	return diff < 0 ? `${text} ago` : `in ${text}`;
}

const LOOP_LABELS: Record<string, string> = {
	health: 'Health checks',
	metrics: 'Container and host metrics',
	uptime: 'Uptime checks and heartbeat',
	alerts: 'Alert rules',
	cron: 'Cron scheduler',
	janitor: 'Retention cleanup',
	'backup-scheduler': 'Scheduled backups',
	'preview-cleanup': 'Preview expiry',
	'proxy-link': 'Proxy connection',
	'route-nudge': 'Route updates',
	'route-poll': 'Route fallback check',
	'route-sync': 'Route reloads',
	certificates: 'Certificates'
};

export function loopLabel(name: string): string {
	return LOOP_LABELS[name] ?? name;
}

export type LoopHealth = 'ok' | 'slow' | 'failing';

/** Scheduler lag above this means the runtime was starved. */
const SLOW_LAG_MS = 1000;

export function loopHealth(loop: Pick<LoopStats, 'last_error' | 'max_lag_ms'>): LoopHealth {
	if (loop.last_error) return 'failing';
	if (loop.max_lag_ms > SLOW_LAG_MS) return 'slow';
	return 'ok';
}

export function loopHealthText(health: LoopHealth): string {
	switch (health) {
		case 'failing':
			return 'Last run failed';
		case 'slow':
			return 'Timers ran late (the server was busy)';
		default:
			return 'Healthy';
	}
}
