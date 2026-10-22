// Presentation helpers for alert rules, live alerts and history.
//
// Kind metadata mirrors tond_common::ALERT_KINDS (the editor also loads the
// server's list for descriptions and availability); values and durations are
// formatted the same way as the notification messages.

import type {
	ActiveAlert,
	AlertComparison,
	AlertEpisode,
	AlertKind,
	AlertRule,
	AlertScope,
	AlertSeverity,
	ChannelQuietHours,
	IntegrationKind,
	QuietHours
} from '$lib/api';

export interface KindMeta {
	label: string;
	scope: AlertScope;
	unit: string;
	comparison: AlertComparison;
	/** Uses a look-back window */
	windowed: boolean;
}

export const ALERT_KIND_META: Record<AlertKind, KindMeta> = {
	app_down: { label: 'App down', scope: 'app', unit: '', comparison: 'none', windowed: false },
	crash_loop: {
		label: 'Crash loop',
		scope: 'app',
		unit: 'restarts',
		comparison: 'at_least',
		windowed: true
	},
	oom: { label: 'Out of memory', scope: 'app', unit: '', comparison: 'none', windowed: true },
	cpu: { label: 'CPU usage', scope: 'app', unit: '%', comparison: 'above', windowed: false },
	memory: { label: 'Memory usage', scope: 'app', unit: '%', comparison: 'above', windowed: false },
	http_5xx_rate: {
		label: '5xx error rate',
		scope: 'app',
		unit: '%',
		comparison: 'above',
		windowed: true
	},
	http_p95_latency: {
		label: 'Slow responses (p95)',
		scope: 'app',
		unit: 'ms',
		comparison: 'above',
		windowed: true
	},
	deploy_failed: {
		label: 'Deploy failed',
		scope: 'app',
		unit: '',
		comparison: 'none',
		windowed: false
	},
	disk_usage: {
		label: 'Disk usage',
		scope: 'host',
		unit: '%',
		comparison: 'above',
		windowed: false
	},
	disk_forecast: {
		label: 'Disk full forecast',
		scope: 'host',
		unit: 'hours',
		comparison: 'below',
		windowed: false
	},
	disk_auto_prune: {
		label: 'Automatic disk cleanup',
		scope: 'host',
		unit: '%',
		comparison: 'none',
		windowed: false
	},
	cert_expiry: {
		label: 'Certificate expiry',
		scope: 'certificate',
		unit: 'days',
		comparison: 'below',
		windowed: false
	},
	backup_failed: {
		label: 'Backup failed',
		scope: 'backup',
		unit: '',
		comparison: 'none',
		windowed: false
	},
	cron_failed: {
		label: 'Cron job failed',
		scope: 'cron',
		unit: '',
		comparison: 'none',
		windowed: false
	}
};

export function kindLabel(kind: string): string {
	return ALERT_KIND_META[kind as AlertKind]?.label ?? kind;
}

/** Groups for the kind picker, in display order. */
export const KIND_GROUPS: { label: string; scopes: AlertScope[] }[] = [
	{ label: 'Apps', scopes: ['app'] },
	{ label: 'Server', scopes: ['host'] },
	{ label: 'Certificates, backups and cron', scopes: ['certificate', 'backup', 'cron'] }
];

/** "93.2%", "1240 ms", "5.5 days", "31 hours", "3 restarts" */
export function formatAlertValue(value: number | null | undefined, unit: string): string {
	if (value === null || value === undefined || Number.isNaN(value)) return '—';
	switch (unit) {
		case '%':
			return `${value.toFixed(1)}%`;
		case 'ms':
			return `${Math.round(value)} ms`;
		case 'days':
		case 'hours':
			return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${unit}`;
		case 'restarts':
			return Math.round(value) === 1 ? '1 restart' : `${Math.round(value)} restarts`;
		default:
			return `${Math.round(value)}`;
	}
}

const COMPARISON_WORD: Record<AlertComparison, string> = {
	above: 'above',
	at_least: 'at least',
	below: 'below',
	none: ''
};

/** "45 s", "12 min", "3 h 5 min", "2 d 4 h" */
export function formatDuration(secs: number): string {
	const s = Math.max(0, Math.floor(secs));
	if (s < 60) return `${s} s`;
	if (s < 3600) return `${Math.floor(s / 60)} min`;
	if (s < 86_400) {
		const h = Math.floor(s / 3600);
		const m = Math.floor((s % 3600) / 60);
		return m ? `${h} h ${m} min` : `${h} h`;
	}
	const d = Math.floor(s / 86_400);
	const h = Math.floor((s % 86_400) / 3600);
	return h ? `${d} d ${h} h` : `${d} d`;
}

/** Seconds between an ISO time and `now` (or `end`). */
export function secondsBetween(from: string, to: string | number = Date.now()): number {
	const end = typeof to === 'number' ? to : new Date(to).getTime();
	return Math.max(0, (end - new Date(from).getTime()) / 1000);
}

const CONDITION_PHRASE: Partial<Record<AlertKind, string>> = {
	app_down: 'App is not running',
	oom: 'Process killed for running out of memory',
	deploy_failed: 'Latest deploy failed',
	backup_failed: 'Latest backup failed',
	cron_failed: 'Latest run failed'
};

/** "CPU usage above 80.0% for 5 min" — the rule's condition in words. */
export function conditionSummary(
	rule: Pick<AlertRule, 'kind' | 'threshold' | 'window_secs' | 'for_secs'>
): string {
	const meta = ALERT_KIND_META[rule.kind];
	if (!meta) return rule.kind;
	let text: string;
	if (meta.comparison === 'none') {
		text = CONDITION_PHRASE[rule.kind] ?? meta.label;
		if (meta.windowed && rule.window_secs) text += ` in ${formatDuration(rule.window_secs)}`;
	} else {
		text = `${meta.label} ${COMPARISON_WORD[meta.comparison]} ${formatAlertValue(rule.threshold, meta.unit)}`;
		if (meta.windowed && rule.window_secs) text += ` in ${formatDuration(rule.window_secs)}`;
	}
	if (rule.for_secs > 0) text += ` for ${formatDuration(rule.for_secs)}`;
	return text;
}

/** "93.2% (alert above 80.0%)" or the condition phrase for yes/no kinds. */
export function valueVsThreshold(
	a: Pick<ActiveAlert | AlertEpisode, 'kind' | 'value' | 'threshold' | 'unit' | 'comparison'>
): string {
	if (a.comparison === 'none') {
		const phrase = CONDITION_PHRASE[a.kind] ?? kindLabel(a.kind);
		return a.kind === 'oom' && a.value && a.value > 1 ? `${phrase} (${a.value}×)` : phrase;
	}
	return `${formatAlertValue(a.value, a.unit)} (alert ${COMPARISON_WORD[a.comparison]} ${formatAlertValue(a.threshold, a.unit)})`;
}

export const SEVERITY_LABEL: Record<AlertSeverity, string> = {
	info: 'Info',
	warning: 'Warning',
	critical: 'Critical'
};

/** Status-badge tone for a severity. */
export function severityTone(severity: string): 'danger' | 'warning' | 'neutral' {
	if (severity === 'critical') return 'danger';
	if (severity === 'warning') return 'warning';
	return 'neutral';
}

export const FOR_OPTIONS: { value: number; label: string }[] = [
	{ value: 0, label: 'Immediately' },
	{ value: 60, label: '1 minute' },
	{ value: 120, label: '2 minutes' },
	{ value: 300, label: '5 minutes' },
	{ value: 600, label: '10 minutes' },
	{ value: 900, label: '15 minutes' },
	{ value: 1800, label: '30 minutes' },
	{ value: 3600, label: '1 hour' }
];

export const WINDOW_OPTIONS: { value: number; label: string }[] = [
	{ value: 60, label: '1 minute' },
	{ value: 300, label: '5 minutes' },
	{ value: 600, label: '10 minutes' },
	{ value: 900, label: '15 minutes' },
	{ value: 1800, label: '30 minutes' },
	{ value: 3600, label: '1 hour' }
];

export const REPEAT_OPTIONS: { value: number; label: string }[] = [
	{ value: 0, label: 'Never' },
	{ value: 1800, label: 'Every 30 minutes' },
	{ value: 3600, label: 'Every hour' },
	{ value: 14_400, label: 'Every 4 hours' },
	{ value: 43_200, label: 'Every 12 hours' },
	{ value: 86_400, label: 'Every day' }
];

/** Snooze choices offered on a live alert. */
export const SNOOZE_OPTIONS: { label: string; minutes?: number; untilResolved?: boolean }[] = [
	{ label: '1 hour', minutes: 60 },
	{ label: '8 hours', minutes: 480 },
	{ label: '24 hours', minutes: 1440 },
	{ label: 'Until resolved', untilResolved: true }
];

/** Label of a preset, or the duration in words for custom values. */
export function optionLabel(options: { value: number; label: string }[], value: number): string {
	return options.find((o) => o.value === value)?.label ?? formatDuration(value);
}

export const CHANNEL_KIND_LABEL: Record<IntegrationKind, string> = {
	slack: 'Slack / Mattermost',
	discord: 'Discord',
	telegram: 'Telegram',
	email: 'Email',
	ntfy: 'ntfy',
	pushover: 'Pushover',
	gotify: 'Gotify',
	webhook: 'Webhook'
};

/** "22:00–07:00 Europe/Berlin", "Off" or "Follows the global quiet hours". */
export function quietHoursSummary(q: ChannelQuietHours, global?: QuietHours): string {
	if (q.mode === 'off') return 'No quiet hours';
	const window = q.mode === 'custom' ? q : global;
	if (!window || !window.enabled) {
		return q.mode === 'global' ? 'Global quiet hours (off)' : 'No quiet hours';
	}
	const text = `Quiet ${window.start}–${window.end} ${window.timezone}`;
	return q.mode === 'global' ? `${text} (global)` : text;
}

/** Where a live alert's target lives in the dashboard. */
export function targetHref(target: string): string | null {
	if (target.startsWith('app:')) return `/apps/${target.slice(4)}`;
	if (target.startsWith('cron:')) return `/cron/${target.slice(5)}`;
	if (target === 'backup') return '/snapshots';
	if (target === 'host') return '/system';
	if (target.startsWith('cert:')) return '/settings';
	return null;
}

/** The browser's IANA time zone (UTC when unknown). */
export function localTimeZone(): string {
	try {
		return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
	} catch {
		return 'UTC';
	}
}
