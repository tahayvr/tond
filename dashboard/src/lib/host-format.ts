// Formatting for the System page: disk forecast, disk breakdown, heartbeat.

import type { DiskEta, DiskUsage, HeartbeatStatus } from '$lib/api';
import { formatBytes } from '$lib/metrics-format';

/** 9 days → "~9 days", 30 hours → "~1 day", 5 hours → "~5 hours". */
export function formatEta(secs: number): string {
	const hours = secs / 3600;
	if (hours < 1) return 'less than an hour';
	if (hours < 36) {
		const h = Math.round(hours);
		return `~${h} hour${h === 1 ? '' : 's'}`;
	}
	const days = Math.round(hours / 24);
	if (days < 60) return `~${days} day${days === 1 ? '' : 's'}`;
	const months = Math.round(days / 30);
	return `~${months} months`;
}

export type ForecastTone = 'danger' | 'warning' | 'success' | 'neutral';

export interface ForecastSummary {
	tone: ForecastTone;
	/** "Full in ~9 days" */
	headline: string;
	/** Which trend and how fast. */
	detail: string;
}

/** Headline and detail for the disk-full forecast. */
export function forecastSummary(f: DiskEta): ForecastSummary {
	const pick = [f.day, f.week]
		.filter((w) => w.eta_secs !== null)
		.sort((a, b) => (a.eta_secs ?? 0) - (b.eta_secs ?? 0))[0];
	if (pick && pick.eta_secs !== null) {
		const days = pick.eta_secs / 86_400;
		return {
			tone: days < 7 ? 'danger' : days < 30 ? 'warning' : 'neutral',
			headline: `Full in ${formatEta(pick.eta_secs)}`,
			detail: `At the last ${pick.window === '24h' ? '24 hours' : '7 days'}' rate: +${formatBytes(pick.growth_per_day)} a day.`
		};
	}
	if (f.day.status === 'insufficient_data' && f.week.status === 'insufficient_data') {
		return {
			tone: 'neutral',
			headline: 'Forecast not ready',
			detail: 'Tond needs a couple of hours of history to tell whether the disk is filling up.'
		};
	}
	const trend = f.day.status === 'stable' ? f.day : f.week;
	return {
		tone: 'success',
		headline: 'Not filling up',
		detail:
			trend.growth_per_day < 0
				? `Usage went down by ${formatBytes(-trend.growth_per_day)} a day recently.`
				: 'Usage is flat (or growing too slowly to matter within a year).'
	};
}

export interface DiskSegment {
	key: string;
	label: string;
	bytes: number;
	/** CSS colour (a theme variable). */
	color: string;
	/** Short explanation for the legend. */
	hint?: string;
}

/**
 * What fills the data directory's filesystem: Docker's parts (when Docker's
 * data root is on it), Tond's data directory, everything else, and free space.
 */
export function diskSegments(disk: DiskUsage, used: number, total: number): DiskSegment[] {
	const segments: DiskSegment[] = [];
	let accounted = 0;
	const add = (s: DiskSegment) => {
		if (s.bytes <= 0) return;
		segments.push(s);
		accounted += s.bytes;
	};
	if (disk.docker_on_data_fs) {
		add({
			key: 'images',
			label: 'Docker images',
			bytes: disk.images.bytes,
			color: 'var(--chart-1)',
			hint: `${disk.images.count} image${disk.images.count === 1 ? '' : 's'}, including rollback versions`
		});
		add({
			key: 'build_cache',
			label: 'Build cache',
			bytes: disk.build_cache.bytes,
			color: 'var(--chart-3)',
			hint: 'Speeds up rebuilds; safe to clear'
		});
		add({
			key: 'containers',
			label: 'Containers',
			bytes: disk.containers.bytes,
			color: 'var(--chart-4)',
			hint: 'Files written inside containers'
		});
		add({
			key: 'volumes',
			label: 'Docker volumes',
			bytes: disk.volumes.bytes,
			color: 'var(--chart-5)'
		});
	}
	add({
		key: 'tond',
		label: 'Tond data',
		bytes: disk.tond_data ?? 0,
		color: 'var(--chart-2)',
		hint: 'Database, app volumes and local backups'
	});
	add({
		key: 'other',
		label: 'Everything else',
		bytes: Math.max(0, used - accounted),
		color: 'var(--muted-foreground)',
		hint: 'The OS, logs and other files'
	});
	segments.push({
		key: 'free',
		label: 'Free',
		bytes: Math.max(0, total - Math.max(used, accounted)),
		color: 'var(--muted)'
	});
	return segments;
}

/** One line about the heartbeat for the services panel. */
export function heartbeatLine(
	h: HeartbeatStatus,
	now = Date.now() / 1000
): {
	tone: 'success' | 'warning' | 'danger' | 'neutral';
	text: string;
} {
	if (!h.configured) return { tone: 'neutral', text: 'Not set up' };
	if (!h.enabled) return { tone: 'neutral', text: 'Turned off' };
	const lastOk = h.last_ok_at ?? 0;
	const lastBad = Math.max(h.last_failure_at ?? 0, h.last_skipped_at ?? 0);
	if (lastBad > lastOk) {
		if ((h.last_skipped_at ?? 0) >= (h.last_failure_at ?? 0))
			return {
				tone: 'warning',
				text: `Holding pings: ${h.last_skip_reason ?? 'Tond is unhealthy'}`
			};
		return { tone: 'danger', text: `Last ping failed: ${h.last_error ?? 'unknown error'}` };
	}
	if (h.last_ok_at) {
		const ago = Math.max(0, Math.round(now - h.last_ok_at));
		const text =
			ago < 90
				? 'just now'
				: ago < 5400
					? `${Math.round(ago / 60)} min ago`
					: `${Math.round(ago / 3600)} h ago`;
		return { tone: 'success', text: `Last ping ${text} to ${h.url_host ?? 'your monitor'}` };
	}
	return { tone: 'neutral', text: 'Waiting for the first ping' };
}
