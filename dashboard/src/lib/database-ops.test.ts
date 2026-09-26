import { describe, expect, it } from 'vitest';
import {
	catalogFor,
	describeBackupSchedule,
	engineLabel,
	parseAllowlist,
	presetFor,
	restoreName,
	upgradeWarnings,
	validDatabaseName
} from './database-ops';
import type { DbCatalogEntry } from './api';

describe('database-ops helpers', () => {
	it('maps schedules to presets and plain English', () => {
		expect(presetFor(null)).toBe('off');
		expect(presetFor('0 3 * * *')).toBe('0 3 * * *');
		expect(presetFor('15 4 * * 1')).toBe('custom');
		expect(describeBackupSchedule(null)).toBeNull();
		expect(describeBackupSchedule('0 */6 * * *')).toBe('Every 6 hours UTC');
		expect(describeBackupSchedule('15 4 * * 1')).toMatch(/04:15.*Monday.*UTC/);
	});

	it('suggests a free name for a restored copy', () => {
		const date = new Date('2026-09-25T10:00:00Z');
		expect(restoreName('shop', date)).toBe('shop-restore-20260925');
		expect(restoreName('shop', date, ['shop-restore-20260925'])).toBe('shop-restore-20260925-2');
		expect(validDatabaseName(restoreName('a'.repeat(80), date))).toBe(true);
	});

	it('validates database names like the server', () => {
		expect(validDatabaseName('shop-copy')).toBe(true);
		expect(validDatabaseName('-shop')).toBe(false);
		expect(validDatabaseName('shop copy')).toBe(false);
		expect(validDatabaseName('')).toBe(false);
	});

	it('labels engines with version and variant', () => {
		expect(engineLabel({ db_type: 'postgres', version: '16', variant: 'pgvector' })).toBe(
			'PostgreSQL 16 · pgvector'
		);
		expect(engineLabel({ db_type: 'keydb', version: 'latest', variant: '' })).toBe('KeyDB');
	});

	it('orders the engine’s own image before its variants', () => {
		const entries = [
			{ db_type: 'postgres', variant: 'postgis' },
			{ db_type: 'mysql', variant: '' },
			{ db_type: 'postgres', variant: '' }
		] as DbCatalogEntry[];
		expect(catalogFor(entries, 'postgres').map((e) => e.variant)).toEqual(['', 'postgis']);
	});

	it('parses allowlists and points at the first bad entry', () => {
		expect(parseAllowlist('203.0.113.7\n198.51.100.0/24, 203.0.113.7')).toEqual({
			entries: ['203.0.113.7', '198.51.100.0/24'],
			invalid: null
		});
		expect(parseAllowlist('203.0.113.7\n2001:db8::1').invalid).toBe('2001:db8::1');
		expect(parseAllowlist('10.0.0.0/33').invalid).toBe('10.0.0.0/33');
		expect(parseAllowlist('256.1.1.1').invalid).toBe('256.1.1.1');
		expect(parseAllowlist('   ')).toEqual({ entries: [], invalid: null });
	});

	it('warns about downtime, rollback and downgrades before an upgrade', () => {
		const w = upgradeWarnings('postgres', '16', '17').join(' ');
		expect(w).toMatch(/lose their connection/);
		expect(w).toMatch(/roll back/);
		expect(w).toMatch(/Downgrades/);
		expect(w).toMatch(/driver/);
	});
});
