// One status vocabulary for the whole dashboard.
//
// Every status the API reports (apps, databases, cron jobs and runs,
// deployments, certificates, previews, snapshots) maps to a tone, an icon and
// a human label here. `StatusBadge` renders the result as colour + icon +
// text, so no status is ever conveyed by colour alone.

/** Semantic tone; each has an AA-contrast colour token in layout.css. */
export type StatusTone = 'success' | 'progress' | 'warning' | 'danger' | 'neutral';

/** What the status describes. `running` means "up" for services but "in progress" for work. */
export type StatusKind =
	| 'app'
	| 'database'
	| 'cron'
	| 'run'
	| 'deployment'
	| 'certificate'
	| 'preview'
	| 'snapshot'
	| 'generic';

export type StatusIcon = 'check' | 'spinner' | 'warning' | 'error' | 'stop' | 'pause' | 'dot';

export interface StatusPresentation {
	tone: StatusTone;
	icon: StatusIcon;
	label: string;
}

const SUCCESS = new Set([
	'active',
	'success',
	'succeeded',
	'completed',
	'healthy',
	'enabled',
	'connected',
	'deployed',
	'ready',
	'installed'
]);
const PROGRESS = new Set([
	'building',
	'deploying',
	'pending',
	'queued',
	'creating',
	'starting',
	'restarting',
	'restoring',
	'provisioning',
	'stopping',
	'in_progress',
	'uploading'
]);
const WARNING = new Set([
	'unhealthy',
	'degraded',
	'expiring',
	'timeout',
	'timed_out',
	'rolled_back',
	'partial',
	'partially_failed',
	'firing',
	'not_installed'
]);
const DANGER = new Set(['failed', 'failure', 'error', 'expired', 'crashed', 'dead', 'invalid']);
const PAUSED = new Set(['paused', 'disabled']);

/** Kinds where `running` is work in progress rather than a healthy service. */
const WORK_KINDS = new Set<StatusKind>(['run', 'deployment', 'snapshot']);

const LABEL_OVERRIDES: Record<string, string> = {
	rolled_back: 'Rolled back',
	timed_out: 'Timed out',
	in_progress: 'In progress',
	not_installed: 'Not installed'
};

/** Turn an API status slug into a sentence-case label ("rolled_back" → "Rolled back"). */
export function statusLabel(status: string | null | undefined): string {
	if (!status) return 'Unknown';
	const key = status.toLowerCase();
	if (LABEL_OVERRIDES[key]) return LABEL_OVERRIDES[key];
	const words = key.replace(/[_-]+/g, ' ').trim();
	return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Map a status to its tone, icon and label. Unknown statuses are neutral. */
export function describeStatus(
	status: string | null | undefined,
	kind: StatusKind = 'generic'
): StatusPresentation {
	const key = (status ?? '').toLowerCase();
	const label = statusLabel(status);

	if (key === 'running') {
		return WORK_KINDS.has(kind)
			? { tone: 'progress', icon: 'spinner', label }
			: { tone: 'success', icon: 'check', label };
	}
	if (SUCCESS.has(key)) return { tone: 'success', icon: 'check', label };
	if (PROGRESS.has(key)) return { tone: 'progress', icon: 'spinner', label };
	if (WARNING.has(key)) return { tone: 'warning', icon: 'warning', label };
	if (DANGER.has(key)) return { tone: 'danger', icon: 'error', label };
	if (PAUSED.has(key)) return { tone: 'neutral', icon: 'pause', label };
	if (key === 'stopped' || key === 'cancelled' || key === 'canceled' || key === 'superseded') {
		return { tone: 'neutral', icon: 'stop', label };
	}
	return { tone: 'neutral', icon: 'dot', label };
}

/** Tailwind classes for each tone: tinted background, AA text colour, matching border. */
export const TONE_CLASSES: Record<StatusTone, string> = {
	success: 'border-success/40 bg-success/10 text-success',
	progress: 'border-info/40 bg-info/10 text-info',
	warning: 'border-warn/40 bg-warn/10 text-warn',
	danger: 'border-destructive/40 bg-destructive/10 text-destructive',
	neutral: 'border-border bg-muted text-muted-foreground'
};
