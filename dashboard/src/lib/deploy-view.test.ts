import { describe, expect, it } from 'vitest';
import type { Deployment } from '$lib/api';
import {
	deploymentSubject,
	failureFix,
	formatDuration,
	isFinished,
	phaseLabel,
	relativeTime,
	secondsBetween,
	statusView,
	triggerLabel,
	visiblePhases
} from './deploy-view';

function dep(overrides: Partial<Deployment> = {}): Deployment {
	return {
		id: 'd1',
		app_id: 'a1',
		image_tag: 'tond-web:abc',
		status: 'succeeded',
		trigger_source: 'manual',
		has_generated_dockerfile: false,
		started_at: '2026-01-01T00:00:00Z',
		finished_at: '2026-01-01T00:01:05Z',
		created_at: '2026-01-01T00:00:00Z',
		image_pruned: false,
		is_current: false,
		rollback_available: false,
		rollback_unavailable_reason: null,
		...overrides
	};
}

describe('statuses', () => {
	it('labels known statuses and degrades gracefully for unknown ones', () => {
		expect(statusView('running')).toEqual({ label: 'Deploying', tone: 'running' });
		expect(statusView('queued', 2)).toEqual({ label: 'Queued (position 2)', tone: 'waiting' });
		expect(statusView('queued')).toEqual({ label: 'Queued', tone: 'waiting' });
		expect(statusView('waiting_for_lock')).toEqual({ label: 'Waiting for lock', tone: 'neutral' });
		expect(isFinished('superseded')).toBe(true);
		expect(isFinished('queued')).toBe(false);
	});
});

describe('durations and times', () => {
	it('formats durations', () => {
		expect(formatDuration(42)).toBe('42s');
		expect(formatDuration(65)).toBe('1m 05s');
		expect(formatDuration(3780)).toBe('1h 03m');
		expect(formatDuration(null)).toBe('—');
	});

	it('measures elapsed time, live while running', () => {
		expect(secondsBetween('2026-01-01T00:00:00Z', '2026-01-01T00:00:30Z')).toBe(30);
		expect(secondsBetween('2026-01-01T00:00:00Z', null, Date.parse('2026-01-01T00:02:00Z'))).toBe(
			120
		);
		expect(secondsBetween(null, null)).toBeNull();
	});

	it('shows relative times', () => {
		const now = Date.parse('2026-01-01T12:00:00Z');
		expect(relativeTime('2026-01-01T11:59:50Z', now)).toBe('just now');
		expect(relativeTime('2026-01-01T11:55:00Z', now)).toBe('5 min ago');
		expect(relativeTime('2026-01-01T09:00:00Z', now)).toBe('3 h ago');
	});
});

describe('labels', () => {
	it('describes triggers', () => {
		expect(triggerLabel({ trigger_source: 'manual', trigger_detail: 'ada@example.com' })).toBe(
			'Manual · ada@example.com'
		);
		expect(triggerLabel({ trigger_source: 'webhook', trigger_detail: 'GitHub push to main' })).toBe(
			'Webhook · GitHub push to main'
		);
		expect(triggerLabel({ trigger_source: 'cli', trigger_detail: 'tond CLI on the server' })).toBe(
			'CLI'
		);
		expect(triggerLabel({ trigger_source: 'nightly', trigger_detail: null })).toBe('nightly');
	});

	it('describes what ran', () => {
		expect(deploymentSubject(dep({ commit_sha: '0123456789', commit_message: 'Fix login' }))).toBe(
			'0123456 Fix login'
		);
		expect(deploymentSubject(dep())).toBe('tond-web:abc');
		expect(deploymentSubject(dep({ image_tag: null }))).toBe('No commit or image recorded');
	});

	it('labels phases and hides skipped ones', () => {
		expect(phaseLabel({ phase: 'health', status: 'pending' })).toBe('Health check');
		expect(phaseLabel({ phase: 'clone', status: 'pending', label: 'Pull image' })).toBe(
			'Pull image'
		);
		expect(
			visiblePhases([
				{ phase: 'clone', status: 'skipped' },
				{ phase: 'build', status: 'running' }
			]).map((p) => p.phase)
		).toEqual(['build']);
	});

	it('points failures at the tab that fixes them', () => {
		expect(failureFix({ code: 'port_not_listening' })?.tab).toBe('health');
		expect(failureFix({ code: 'app_oom' })?.tab).toBe('resources');
		expect(failureFix({ code: 'image_pull_denied' })?.tab).toBe('env');
		expect(failureFix({ code: 'build_failed' })).toBeNull();
		expect(failureFix(null)).toBeNull();
	});
});
