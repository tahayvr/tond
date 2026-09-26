import { describe, expect, it } from 'vitest';

import {
	deployProgressMessage,
	deploymentStatusView,
	isUnfinished,
	ordinal
} from './deploymentStatus';

describe('ordinal', () => {
	it('uses the right English suffix', () => {
		expect([1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111].map(ordinal)).toEqual([
			'1st',
			'2nd',
			'3rd',
			'4th',
			'11th',
			'12th',
			'13th',
			'21st',
			'22nd',
			'101st',
			'111th'
		]);
	});
});

describe('deploymentStatusView', () => {
	it('shows the place in line for queued deployments', () => {
		expect(deploymentStatusView({ status: 'queued', queue_position: 3, error: null })).toEqual({
			label: 'Queued',
			variant: 'outline',
			detail: '3rd in line',
			active: true
		});
		expect(deploymentStatusView({ status: 'queued', queue_position: 1, error: null }).detail).toBe(
			'Next in line'
		);
		expect(
			deploymentStatusView({ status: 'queued', queue_position: null, error: null }).detail
		).toBe('Next in line');
	});

	it('marks in-flight states active and final states inactive', () => {
		for (const status of ['building', 'deploying'] as const) {
			expect(deploymentStatusView({ status, queue_position: null, error: null }).active).toBe(true);
		}
		for (const status of ['succeeded', 'failed', 'superseded'] as const) {
			expect(deploymentStatusView({ status, queue_position: null, error: null }).active).toBe(
				false
			);
		}
	});

	it('explains failures and supersession', () => {
		const failed = deploymentStatusView({
			status: 'failed',
			queue_position: null,
			error: 'Interrupted: Tond restarted while this was in progress. Deploy again to retry.'
		});
		expect(failed.variant).toBe('destructive');
		expect(failed.detail).toContain('Deploy again');
		expect(
			deploymentStatusView({ status: 'superseded', queue_position: null, error: null }).detail
		).toBe('Replaced by a newer deploy before it started');
	});
});

describe('isUnfinished', () => {
	it('is true only while a deployment can still change', () => {
		expect(['queued', 'building', 'deploying'].every(isUnfinished)).toBe(true);
		expect(['succeeded', 'failed', 'superseded'].some(isUnfinished)).toBe(false);
	});
});

describe('deployProgressMessage', () => {
	it('reads naturally for each stage', () => {
		expect(deployProgressMessage({ status: 'queued', queue_position: 2, kind: 'deploy' })).toBe(
			'Deploy queued: 2nd in line'
		);
		expect(deployProgressMessage({ status: 'queued', queue_position: 1, kind: 'rollback' })).toBe(
			'Rollback queued: starting shortly'
		);
		expect(
			deployProgressMessage({ status: 'building', queue_position: null, kind: 'deploy' })
		).toBe('Building the image…');
		expect(
			deployProgressMessage({ status: 'succeeded', queue_position: null, kind: 'deploy' })
		).toBe('');
	});
});
