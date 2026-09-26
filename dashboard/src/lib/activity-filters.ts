// activity-filters.ts — Activity page filters <-> URL <-> audit-log query
//
// Filters live in the URL (?actor=…&action=…) so a filtered view can be
// shared or reloaded, and the page's load function fetches exactly that view.
// Filtering happens on the server, before pagination.

import type { AuditLogQuery } from './api';

export interface ActivityFilters {
	q: string;
	actor: string;
	/** An action or action prefix ending in `*`, or '' for all */
	action: string;
	resourceType: string;
	failuresOnly: boolean;
}

export const EMPTY_FILTERS: ActivityFilters = {
	q: '',
	actor: '',
	action: '',
	resourceType: '',
	failuresOnly: false
};

/** Event categories offered in the filter (server-side action prefixes). */
export const ACTION_CATEGORIES: { value: string; label: string }[] = [
	{ value: '', label: 'All events' },
	{ value: 'login*', label: 'Sign-ins' },
	{ value: 'user_*', label: 'Users and roles' },
	{ value: 'token_*', label: 'API tokens' },
	{ value: 'session_*', label: 'Sessions' },
	{ value: '2fa_*', label: 'Two-factor auth' },
	{ value: 'password*', label: 'Passwords' },
	{ value: 'deploy', label: 'Deploys' },
	{ value: 'env_*', label: 'Env vars' },
	{ value: 'terminal_*', label: 'Terminal' },
	{ value: 'snapshot_*', label: 'Snapshots and restores' },
	{ value: 'settings_update', label: 'Server settings' }
];

export const RESOURCE_TYPES: { value: string; label: string }[] = [
	{ value: '', label: 'Anything' },
	{ value: 'app', label: 'Apps' },
	{ value: 'database', label: 'Databases' },
	{ value: 'project', label: 'Projects' },
	{ value: 'user', label: 'Users' },
	{ value: 'token', label: 'API tokens' },
	{ value: 'snapshot', label: 'Snapshots' },
	{ value: 'settings', label: 'Settings' }
];

/** Page size for the activity list. */
export const ACTIVITY_PAGE_SIZE = 50;

export function filtersFromUrl(url: URL): ActivityFilters {
	const p = url.searchParams;
	return {
		q: p.get('q') ?? '',
		actor: p.get('actor') ?? '',
		action: p.get('action') ?? '',
		resourceType: p.get('resource_type') ?? '',
		failuresOnly: p.get('outcome') === 'failure'
	};
}

/** The URL search string for a set of filters ('' when none are set). */
export function filtersToSearch(filters: ActivityFilters): string {
	const p = new URLSearchParams();
	if (filters.q.trim()) p.set('q', filters.q.trim());
	if (filters.actor.trim()) p.set('actor', filters.actor.trim());
	if (filters.action) p.set('action', filters.action);
	if (filters.resourceType) p.set('resource_type', filters.resourceType);
	if (filters.failuresOnly) p.set('outcome', 'failure');
	const s = p.toString();
	return s ? `?${s}` : '';
}

/** The audit-log API query for a set of filters. */
export function filtersToQuery(filters: ActivityFilters, before?: number): AuditLogQuery {
	return {
		q: filters.q.trim() || undefined,
		actor: filters.actor.trim() || undefined,
		action: filters.action || undefined,
		resource_type: filters.resourceType || undefined,
		outcome: filters.failuresOnly ? 'failure' : undefined,
		before,
		limit: ACTIVITY_PAGE_SIZE
	};
}

export function hasFilters(filters: ActivityFilters): boolean {
	return filtersToSearch(filters) !== '';
}
