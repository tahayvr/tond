// App event feed helpers: categories for filtering and day grouping.

import type { AppEvent, AppEventKind } from '$lib/api';

export type EventCategory = 'deploys' | 'runtime' | 'changes';

export const EVENT_CATEGORIES: { id: EventCategory | 'all'; label: string }[] = [
	{ id: 'all', label: 'All' },
	{ id: 'deploys', label: 'Deploys' },
	{ id: 'runtime', label: 'Health' },
	{ id: 'changes', label: 'Changes' }
];

export function eventCategory(kind: AppEventKind): EventCategory {
	switch (kind) {
		case 'deploy_started':
		case 'deploy_succeeded':
		case 'deploy_failed':
		case 'deploy_cancelled':
			return 'deploys';
		case 'restart':
		case 'crash':
		case 'oom':
		case 'cert_issued':
		case 'cert_failed':
		case 'alert_fired':
		case 'alert_resolved':
		case 'uptime_down':
		case 'uptime_up':
			return 'runtime';
		default:
			return 'changes';
	}
}

export function filterEvents(events: AppEvent[], category: EventCategory | 'all'): AppEvent[] {
	return category === 'all' ? events : events.filter((e) => eventCategory(e.kind) === category);
}

/** "Today", "Yesterday", or a date like "Mon, Sep 22". */
export function dayLabel(iso: string, now = Date.now()): string {
	const d = new Date(iso);
	const today = new Date(now);
	const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
	const days = Math.round((startOf(today) - startOf(d)) / 86_400_000);
	if (days === 0) return 'Today';
	if (days === 1) return 'Yesterday';
	return d.toLocaleDateString(undefined, {
		weekday: 'short',
		month: 'short',
		day: 'numeric',
		year: d.getFullYear() === today.getFullYear() ? undefined : 'numeric'
	});
}

/** Consecutive events grouped under their day label (input newest first). */
export function groupByDay(
	events: AppEvent[],
	now = Date.now()
): { day: string; events: AppEvent[] }[] {
	const groups: { day: string; events: AppEvent[] }[] = [];
	for (const e of events) {
		const day = dayLabel(e.at, now);
		const last = groups[groups.length - 1];
		if (last && last.day === day) last.events.push(e);
		else groups.push({ day, events: [e] });
	}
	return groups;
}

/** Count of problems (crashes, OOM kills, failed deploys) among `events`. */
export function problemCount(events: AppEvent[]): number {
	return events.filter((e) => e.severity === 'danger').length;
}
