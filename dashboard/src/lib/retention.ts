// Retention settings copy and the database-size breakdown.

import type { RetentionField, TableSize } from '$lib/api';

export interface RetentionCopy {
	label: string;
	help: string;
	group: RetentionGroup;
}

export type RetentionGroup = 'deploys' | 'cron' | 'events' | 'security';

export const RETENTION_GROUPS: { id: RetentionGroup; title: string }[] = [
	{ id: 'deploys', title: 'Deployments' },
	{ id: 'cron', title: 'Cron jobs' },
	{ id: 'events', title: 'Events and alerts' },
	{ id: 'security', title: 'Security' }
];

/** Labels and help text per settings key (unknown keys fall back to the key). */
export const RETENTION_COPY: Record<string, RetentionCopy> = {
	retention_deploy_logs_deployments: {
		label: 'Build logs of the last',
		help: 'Per app. The latest deploy always keeps its log.',
		group: 'deploys'
	},
	retention_deploy_logs_days: {
		label: 'Build logs for at most',
		help: 'Older build logs are removed even if they are within the count above.',
		group: 'deploys'
	},
	retention_deployments_per_app: {
		label: 'Deployment history per app',
		help: 'Running deploys and rollback targets are never removed.',
		group: 'deploys'
	},
	retention_cron_runs_per_job: {
		label: 'Runs of the last',
		help: 'Per job, with their output. The latest run is always kept.',
		group: 'cron'
	},
	retention_cron_runs_days: {
		label: 'Runs for at most',
		help: 'Older runs are removed even if they are within the count above.',
		group: 'cron'
	},
	retention_app_events_days: {
		label: 'Restarts, crashes and OOM kills for',
		help: 'Shown in each app’s Recent events and metrics.',
		group: 'events'
	},
	retention_alert_history_days: {
		label: 'Resolved alerts for',
		help: 'Alert history on the Alerts page and in Recent events.',
		group: 'events'
	},
	retention_uptime_days: {
		label: 'Hourly uptime history and incidents for',
		help: 'Each probe is kept 48 hours; the hourly summaries behind the 7- and 30-day uptime and the 90-day bars are kept this long.',
		group: 'events'
	},
	retention_request_metrics_days: {
		label: 'Hourly request metrics for',
		help: 'Requests, errors, latency and bandwidth per app behind the 7- to 90-day charts. Minute-level rows are kept 48 hours.',
		group: 'events'
	},
	retention_host_history_days: {
		label: 'Hourly host history for',
		help: 'CPU, load, memory and disk on the System page. Minute-level rows are kept 48 hours.',
		group: 'events'
	},
	retention_audit_log_days: {
		label: 'Activity (audit) log for',
		help: 'Removing old entries keeps the hash chain verifiable. At least 30 days.',
		group: 'security'
	}
};

export function retentionCopy(key: string): RetentionCopy {
	return RETENTION_COPY[key] ?? { label: key, help: '', group: 'events' };
}

/** "180 days" / "1 day" / "30 deploys". */
export function formatUnit(value: number, unit: string): string {
	if (value === 1) return `1 ${unit.replace(/s$/, '')}`;
	return `${value} ${unit}`;
}

/** Why a value can't be saved, or null if it's fine. */
export function validateRetention(field: RetentionField, value: number | null): string | null {
	if (value === null || !Number.isInteger(value)) return 'Enter a whole number.';
	if (value < field.min || value > field.max) {
		return `Between ${field.min} and ${formatUnit(field.max, field.unit)}.`;
	}
	return null;
}

/** Only the fields whose value changed. */
export function retentionChanges(
	fields: RetentionField[],
	values: Record<string, number | null>
): Record<string, number> {
	const changes: Record<string, number> = {};
	for (const f of fields) {
		const v = values[f.key];
		if (v !== null && v !== undefined && v !== f.value) changes[f.key] = v;
	}
	return changes;
}

const TABLE_GROUPS: Record<string, string> = {
	deploy_logs: 'Build logs',
	deployments: 'Deployment history',
	cron_job_runs: 'Cron runs',
	audit_log: 'Activity log',
	audit_outbox: 'Activity log',
	metrics_raw: 'Metrics',
	metrics_5m: 'Metrics',
	metrics_1h: 'Metrics',
	metric_targets: 'Metrics',
	metric_events: 'App events',
	alert_history: 'Alert history',
	alert_deliveries: 'Alert history',
	sessions: 'Sessions',
	snapshots: 'Snapshot records',
	webhook_deliveries: 'Webhook deliveries'
};

/** Which part of Tond a table belongs to ("metrics_5m" → "Metrics"). */
export function tableGroup(name: string): string {
	if (TABLE_GROUPS[name]) return TABLE_GROUPS[name];
	if (name.startsWith('metrics_') || name.startsWith('request_metrics')) return 'Metrics';
	return 'Other';
}

export interface SizeRow {
	label: string;
	bytes: number;
	/** Share of `total`, 0–100. */
	percent: number;
}

/**
 * Sum tables into their groups, largest first; groups beyond `top` (and
 * "Other") are folded into one "Everything else" row at the end.
 */
export function sizeBreakdown(tables: TableSize[], top = 6): SizeRow[] {
	const sums = new Map<string, number>();
	for (const t of tables) {
		const g = tableGroup(t.name);
		sums.set(g, (sums.get(g) ?? 0) + t.bytes);
	}
	const total = [...sums.values()].reduce((a, b) => a + b, 0);
	const pct = (b: number) => (total > 0 ? (b / total) * 100 : 0);
	const named = [...sums.entries()].filter(([g]) => g !== 'Other').sort((a, b) => b[1] - a[1]);
	const shown = named.slice(0, top);
	const rest = named.slice(top).reduce((a, [, b]) => a + b, 0) + (sums.get('Other') ?? 0);
	const rows = shown.map(([label, bytes]) => ({ label, bytes, percent: pct(bytes) }));
	if (rest > 0) rows.push({ label: 'Everything else', bytes: rest, percent: pct(rest) });
	return rows;
}
