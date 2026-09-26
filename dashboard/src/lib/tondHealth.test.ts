import { describe, expect, it } from 'vitest';

import {
	formatBytes,
	formatMs,
	formatPercent,
	formatUptime,
	loopHealth,
	loopHealthText,
	loopLabel,
	relativeTime
} from './tondHealth';

describe('formatting', () => {
	it('formats bytes, percentages and durations compactly', () => {
		expect(formatBytes(null)).toBe('–');
		expect(formatBytes(512)).toBe('512 B');
		expect(formatBytes(32 * 1024)).toBe('32 KB');
		expect(formatBytes(14.25 * 1024 * 1024)).toBe('14.3 MB');
		expect(formatPercent(0.04)).toBe('<0.1%');
		expect(formatPercent(0)).toBe('0.0%');
		expect(formatPercent(12.345)).toBe('12.3%');
		expect(formatPercent(null)).toBe('–');
		expect(formatMs(12.4)).toBe('12 ms');
		expect(formatMs(4200)).toBe('4.2 s');
		expect(formatMs(180_000)).toBe('3 min');
	});

	it('formats uptime', () => {
		expect(formatUptime(45)).toBe('45 s');
		expect(formatUptime(12 * 60)).toBe('12 min');
		expect(formatUptime(3 * 3600 + 5 * 60)).toBe('3 h 5 min');
		expect(formatUptime(2 * 86_400 + 4 * 3600)).toBe('2 d 4 h');
		expect(formatUptime(86_400)).toBe('1 d');
	});

	it('describes times relative to now', () => {
		const now = Date.parse('2026-05-01T12:00:00Z');
		expect(relativeTime(null, now)).toBe('never');
		expect(relativeTime('2026-05-01T11:59:58Z', now)).toBe('just now');
		expect(relativeTime('2026-05-01T11:57:00Z', now)).toBe('3 min ago');
		expect(relativeTime('2026-05-01T12:58:00Z', now)).toBe('in 58 min');
		expect(relativeTime('2026-05-01T14:00:00Z', now)).toBe('in 2 h');
		expect(relativeTime('2026-04-29T12:00:00Z', now)).toBe('2 d ago');
	});
});

describe('loops', () => {
	it('uses friendly names and falls back to the raw one', () => {
		expect(loopLabel('health')).toBe('Health checks');
		expect(loopLabel('route-poll')).toBe('Route fallback check');
		expect(loopLabel('something-new')).toBe('something-new');
	});

	it('flags failures before lag', () => {
		expect(loopHealth({ last_error: 'db locked', max_lag_ms: 5000 })).toBe('failing');
		expect(loopHealth({ last_error: null, max_lag_ms: 1500 })).toBe('slow');
		expect(loopHealth({ last_error: null, max_lag_ms: 3 })).toBe('ok');
		expect(loopHealthText('failing')).toBe('Last run failed');
	});
});
