import { describe, expect, it } from 'vitest';
import {
	barTone,
	barsSummary,
	causeLabel,
	formatDuration,
	formatUptime,
	relativeUnix,
	validateCheckForm
} from './uptime';

describe('uptime formatting', () => {
	it('formats percentages without rounding up to 100%', () => {
		expect(formatUptime(null)).toBe('—');
		expect(formatUptime(100)).toBe('100%');
		expect(formatUptime(99.996)).toBe('99.99%');
		expect(formatUptime(99.5)).toBe('99.50%');
		expect(formatUptime(97.38)).toBe('97.3%');
	});

	it('colours bars by the share of passed checks', () => {
		expect(barTone({ probes: 0, up: 0 })).toBe('none');
		expect(barTone({ probes: 30, up: 30 })).toBe('up');
		expect(barTone({ probes: 30, up: 28 })).toBe('partial');
		expect(barTone({ probes: 30, up: 3 })).toBe('down');
		expect(barsSummary([])).toBe('No checks in this period');
		expect(
			barsSummary([
				{ t: 0, probes: 30, up: 30 },
				{ t: 1, probes: 30, up: 10 }
			])
		).toBe('1 of 2 periods had failed checks');
	});

	it('labels causes and durations', () => {
		expect(causeLabel('http_5xx')).toBe('Server error (5xx)');
		expect(causeLabel(null)).toBe('');
		expect(formatDuration(45)).toBe('45 s');
		expect(formatDuration(180)).toBe('3 min');
		expect(formatDuration(4380)).toBe('1 h 13 min');
		expect(formatDuration(190_000)).toBe('2 d 4 h');
		expect(relativeUnix(100, 100)).toBe('just now');
		expect(relativeUnix(40, 220)).toBe('3 min ago');
		expect(relativeUnix(260, 220)).toBe('in 40 s');
		expect(relativeUnix(null, 0)).toBe('never');
	});

	it('validates the check form', () => {
		const ok = { path: '/health', expectedStatus: '', timeoutSecs: 10 };
		expect(validateCheckForm(ok)).toBe('');
		expect(validateCheckForm({ ...ok, path: 'health' })).toMatch(/start with \//);
		expect(validateCheckForm({ ...ok, path: '/a b' })).toMatch(/spaces/);
		expect(validateCheckForm({ ...ok, expectedStatus: '42' })).toMatch(/status code/);
		expect(validateCheckForm({ ...ok, expectedStatus: '204' })).toBe('');
		expect(validateCheckForm({ ...ok, timeoutSecs: 0 })).toMatch(/timeout/);
	});
});
