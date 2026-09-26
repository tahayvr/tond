import { describe, expect, it } from 'vitest';
import type { RetentionField } from '$lib/api';
import {
	formatUnit,
	retentionChanges,
	retentionCopy,
	sizeBreakdown,
	tableGroup,
	validateRetention
} from './retention';

const field: RetentionField = {
	key: 'retention_audit_log_days',
	value: 180,
	default: 180,
	min: 30,
	max: 3650,
	unit: 'days'
};

describe('retention copy and validation', () => {
	it('labels known keys and falls back for unknown ones', () => {
		expect(retentionCopy('retention_audit_log_days').group).toBe('security');
		expect(retentionCopy('retention_new_thing').label).toBe('retention_new_thing');
	});

	it('formats units', () => {
		expect(formatUnit(1, 'days')).toBe('1 day');
		expect(formatUnit(30, 'deploys')).toBe('30 deploys');
	});

	it('checks bounds and whole numbers', () => {
		expect(validateRetention(field, 365)).toBeNull();
		expect(validateRetention(field, 10)).toBe('Between 30 and 3650 days.');
		expect(validateRetention(field, 40.5)).toBe('Enter a whole number.');
		expect(validateRetention(field, null)).toBe('Enter a whole number.');
	});

	it('sends only changed values', () => {
		expect(retentionChanges([field], { retention_audit_log_days: 180 })).toEqual({});
		expect(retentionChanges([field], { retention_audit_log_days: 365 })).toEqual({
			retention_audit_log_days: 365
		});
	});
});

describe('sizeBreakdown', () => {
	it('groups tables, sorts by size and folds the rest', () => {
		expect(tableGroup('metrics_5m')).toBe('Metrics');
		expect(tableGroup('apps')).toBe('Other');
		const rows = sizeBreakdown(
			[
				{ name: 'deploy_logs', bytes: 50 },
				{ name: 'metrics_raw', bytes: 10 },
				{ name: 'metrics_1h', bytes: 20 },
				{ name: 'audit_log', bytes: 10 },
				{ name: 'apps', bytes: 5 },
				{ name: 'sessions', bytes: 5 }
			],
			2
		);
		expect(rows.map((r) => r.label)).toEqual(['Build logs', 'Metrics', 'Everything else']);
		expect(rows[1].bytes).toBe(30);
		expect(rows[2].bytes).toBe(20);
		expect(Math.round(rows.reduce((a, r) => a + r.percent, 0))).toBe(100);
	});

	it('is empty without tables', () => {
		expect(sizeBreakdown([])).toEqual([]);
	});
});
