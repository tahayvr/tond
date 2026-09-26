// Pure helpers for deployments: status and trigger labels, durations, step
// progress and where a failure's fix lives. Shared by the live deploy view
// and the deployments tab.

import type { DeployFailure, DeployPhase, Deployment, PhaseRecord } from '$lib/api';
import type { AppSettingsSection } from '$lib/tabs';

export type Tone = 'success' | 'failure' | 'running' | 'waiting' | 'neutral';

/** Statuses after which nothing changes any more. */
export function isFinished(status: string): boolean {
	return ['succeeded', 'failed', 'superseded', 'cancelled', 'canceled'].includes(status);
}

/** Label and tone for a deployment status; unknown statuses are shown as-is. */
export function statusView(
	status: string,
	queuePosition?: number | null
): { label: string; tone: Tone } {
	switch (status) {
		case 'succeeded':
			return { label: 'Succeeded', tone: 'success' };
		case 'failed':
			return { label: 'Failed', tone: 'failure' };
		case 'running':
			return { label: 'Deploying', tone: 'running' };
		case 'pending':
			return { label: 'Starting', tone: 'waiting' };
		case 'queued':
			return {
				label: queuePosition ? `Queued (position ${queuePosition})` : 'Queued',
				tone: 'waiting'
			};
		case 'superseded':
			return { label: 'Superseded by a newer deploy', tone: 'neutral' };
		case 'cancelled':
		case 'canceled':
			return { label: 'Cancelled', tone: 'neutral' };
		default: {
			const words = status.replace(/[_-]+/g, ' ').trim();
			return {
				label: words ? words[0].toUpperCase() + words.slice(1) : 'Unknown',
				tone: 'neutral'
			};
		}
	}
}

const DEFAULT_PHASE_LABELS: Record<DeployPhase, string> = {
	clone: 'Clone',
	build: 'Build',
	start: 'Start',
	health: 'Health check',
	release: 'Switch traffic'
};

export function phaseLabel(p: PhaseRecord): string {
	return p.label || DEFAULT_PHASE_LABELS[p.phase] || p.phase;
}

/** The steps to show (skipped ones hidden). */
export function visiblePhases(phases: PhaseRecord[] | undefined): PhaseRecord[] {
	return (phases ?? []).filter((p) => p.status !== 'skipped');
}

/** "42s", "1m 05s", "1h 03m". */
export function formatDuration(totalSecs: number | null | undefined): string {
	if (totalSecs == null || !Number.isFinite(totalSecs) || totalSecs < 0) return '—';
	const s = Math.floor(totalSecs);
	if (s < 60) return `${s}s`;
	const m = Math.floor(s / 60);
	if (m < 60) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
	const h = Math.floor(m / 60);
	return `${h}h ${String(m % 60).padStart(2, '0')}m`;
}

/** Seconds from `start` to `end` (or `now` while running). */
export function secondsBetween(
	start: string | undefined | null,
	end: string | undefined | null,
	now: number = Date.now()
): number | null {
	if (!start) return null;
	const a = new Date(start).getTime();
	const b = end ? new Date(end).getTime() : now;
	if (Number.isNaN(a) || Number.isNaN(b)) return null;
	return Math.max(0, Math.round((b - a) / 1000));
}

/** "Manual · ada@example.com", "Webhook · GitHub push to main", "Rollback". */
export function triggerLabel(d: Pick<Deployment, 'trigger_source' | 'trigger_detail'>): string {
	const names: Record<string, string> = {
		manual: 'Manual',
		webhook: 'Webhook',
		rollback: 'Rollback',
		upload: 'Upload',
		cli: 'CLI',
		api: 'API',
		cron: 'Schedule'
	};
	const base = names[d.trigger_source] ?? d.trigger_source;
	if (!d.trigger_detail) return base;
	// The CLI/API details only restate the source.
	if (d.trigger_source === 'cli' || d.trigger_source === 'api') return base;
	return `${base} · ${d.trigger_detail}`;
}

export function shortSha(sha: string | null | undefined): string {
	return sha ? sha.slice(0, 7) : '';
}

/** One line describing what a deployment ran: commit, else image tag. */
export function deploymentSubject(d: Deployment): string {
	if (d.commit_sha) {
		return d.commit_message
			? `${shortSha(d.commit_sha)} ${d.commit_message}`
			: shortSha(d.commit_sha);
	}
	if (d.image_tag) return d.image_tag;
	return 'No commit or image recorded';
}

/** Where the fix for a failure lives on the app page, if anywhere. */
export function failureFix(
	failure: Pick<DeployFailure, 'code'> | null | undefined
): { section: AppSettingsSection; label: string } | null {
	switch (failure?.code) {
		case 'port_not_listening':
		case 'health_check_failed':
		case 'health_check_timeout':
			return { section: 'health', label: 'Open port and health settings' };
		case 'app_crashed':
			return { section: 'environment', label: 'Check environment variables' };
		case 'app_oom':
			return { section: 'resources', label: 'Raise the memory limit' };
		case 'image_pull_denied':
			return { section: 'environment', label: 'Add registry credentials' };
		case 'image_not_found':
		case 'repo_access_denied':
		case 'branch_not_found':
		case 'clone_failed':
		case 'no_dockerfile':
		case 'build_detection_failed':
		case 'bad_base_dir':
			return { section: 'git', label: 'Open source settings' };
		default:
			return null;
	}
}

/** Relative time: "just now", "5 min ago", "3 h ago", else a date. */
export function relativeTime(iso: string, now: number = Date.now()): string {
	const t = new Date(iso).getTime();
	if (Number.isNaN(t)) return iso;
	const secs = Math.round((now - t) / 1000);
	if (secs < 45) return 'just now';
	const mins = Math.round(secs / 60);
	if (mins < 60) return `${mins} min ago`;
	const hours = Math.round(mins / 60);
	if (hours < 24) return `${hours} h ago`;
	const days = Math.round(hours / 24);
	if (days < 7) return `${days} d ago`;
	return new Date(t).toLocaleDateString();
}
