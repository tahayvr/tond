import { describe, expect, it } from 'vitest';
import type { AppEvent } from '$lib/api';
import { dayLabel, eventCategory, filterEvents, groupByDay, problemCount } from './app-events';

const NOW = new Date(2026, 8, 25, 12, 0, 0).getTime();

function ev(kind: AppEvent['kind'], at: Date, severity: AppEvent['severity'] = 'info'): AppEvent {
	return {
		at: at.toISOString(),
		kind,
		severity,
		title: kind,
		detail: null,
		actor: null,
		deployment_id: null
	};
}

describe('app events', () => {
	it('sorts kinds into categories and filters by them', () => {
		expect(eventCategory('deploy_failed')).toBe('deploys');
		expect(eventCategory('oom')).toBe('runtime');
		expect(eventCategory('cert_failed')).toBe('runtime');
		expect(eventCategory('domain_changed')).toBe('changes');
		const list = [ev('deploy_succeeded', new Date(NOW)), ev('crash', new Date(NOW))];
		expect(filterEvents(list, 'all')).toHaveLength(2);
		expect(filterEvents(list, 'runtime').map((e) => e.kind)).toEqual(['crash']);
	});

	it('labels days and groups consecutive events', () => {
		expect(dayLabel(new Date(2026, 8, 25, 1).toISOString(), NOW)).toBe('Today');
		expect(dayLabel(new Date(2026, 8, 24, 23).toISOString(), NOW)).toBe('Yesterday');
		expect(dayLabel(new Date(2026, 8, 20).toISOString(), NOW)).not.toMatch(/Today|Yesterday/);
		const groups = groupByDay(
			[
				ev('restart', new Date(2026, 8, 25, 10)),
				ev('crash', new Date(2026, 8, 25, 9)),
				ev('oom', new Date(2026, 8, 24, 9))
			],
			NOW
		);
		expect(groups.map((g) => [g.day, g.events.length])).toEqual([
			['Today', 2],
			['Yesterday', 1]
		]);
	});

	it('counts problems', () => {
		expect(
			problemCount([
				ev('crash', new Date(NOW), 'danger'),
				ev('restart', new Date(NOW), 'warning'),
				ev('deploy_failed', new Date(NOW), 'danger')
			])
		).toBe(2);
	});
});
