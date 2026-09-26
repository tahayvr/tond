// How a deployment's state reads in the dashboard.
//
// Deployments move queued → building → deploying → succeeded | failed, or
// queued → superseded when a newer deploy of the same app replaced them
// before they started (see crates/tond-server/src/deploy/queue.rs).

import type { Deployment, DeploymentStatus } from '$lib/api';

export type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline';

export interface DeploymentStatusView {
	/** Short badge text */
	label: string;
	variant: BadgeVariant;
	/** One line of context (queue position, failure reason), or null */
	detail: string | null;
	/** Still moving: show progress and keep refreshing */
	active: boolean;
}

const UNFINISHED: ReadonlySet<string> = new Set<DeploymentStatus>([
	'queued',
	'building',
	'deploying'
]);

export function isUnfinished(status: string): boolean {
	return UNFINISHED.has(status);
}

/** "1st", "2nd", "3rd", "4th", … "11th", "12th", "13th", "21st" */
export function ordinal(n: number): string {
	const mod100 = n % 100;
	if (mod100 >= 11 && mod100 <= 13) return `${n}th`;
	switch (n % 10) {
		case 1:
			return `${n}st`;
		case 2:
			return `${n}nd`;
		case 3:
			return `${n}rd`;
		default:
			return `${n}th`;
	}
}

function queueDetail(position: number | null): string {
	if (!position || position <= 1) return 'Next in line';
	return `${ordinal(position)} in line`;
}

export function deploymentStatusView(
	d: Pick<Deployment, 'status' | 'queue_position' | 'error'>
): DeploymentStatusView {
	switch (d.status) {
		case 'queued':
			return {
				label: 'Queued',
				variant: 'outline',
				detail: queueDetail(d.queue_position),
				active: true
			};
		case 'building':
			return { label: 'Building', variant: 'secondary', detail: null, active: true };
		case 'deploying':
			return {
				label: 'Deploying',
				variant: 'secondary',
				detail: 'Starting the new container',
				active: true
			};
		case 'succeeded':
			return { label: 'Succeeded', variant: 'default', detail: null, active: false };
		case 'failed':
			return { label: 'Failed', variant: 'destructive', detail: d.error, active: false };
		case 'superseded':
			return {
				label: 'Superseded',
				variant: 'outline',
				detail: 'Replaced by a newer deploy before it started',
				active: false
			};
		default:
			return { label: d.status, variant: 'outline', detail: d.error, active: false };
	}
}

/** Sentence for the app header while a deploy is on its way. */
export function deployProgressMessage(
	d: Pick<Deployment, 'status' | 'queue_position' | 'kind'>
): string {
	const what = d.kind === 'rollback' ? 'Rollback' : 'Deploy';
	switch (d.status) {
		case 'queued':
			return d.queue_position && d.queue_position > 1
				? `${what} queued: ${ordinal(d.queue_position)} in line`
				: `${what} queued: starting shortly`;
		case 'building':
			return 'Building the image…';
		case 'deploying':
			return 'Starting the new container and switching traffic…';
		default:
			return '';
	}
}
