import { describe, expect, it } from 'vitest';
import { browserTimeZone, describeCron, timeZones } from './cron-schedule';

describe('describeCron', () => {
	it.each([
		['* * * * *', 'Every minute'],
		['*/5 * * * *', 'Every 5 minutes'],
		['0 * * * *', 'Every hour'],
		['15 * * * *', 'At 15 minutes past every hour'],
		['0 */6 * * *', 'At 0 minutes past every 6 hours'],
		['0 2 * * *', 'At 02:00'],
		['30 9 * * 1-5', 'At 09:30, on weekdays'],
		['0 0 * * 0', 'At 00:00, on Sunday'],
		['0 0 * * 7', 'At 00:00, on Sunday'],
		['0 0 * * mon,fri', 'At 00:00, on Monday and Friday'],
		['0 0 1 * *', 'At 00:00, on the 1st'],
		['0 12 1,15 * *', 'At 12:00, on the 1st and 15th'],
		['0 0 1 1 *', 'At 00:00, on the 1st, in January'],
		['0 8,18 * * *', 'At 08:00 and 18:00'],
		['@daily', 'At 00:00'],
		['@hourly', 'Every hour']
	])('%s → %s', (expr, text) => {
		expect(describeCron(expr)).toEqual({ ok: true, text });
	});

	it.each([
		['', 'Enter a schedule'],
		['0 0 * *', 'A schedule needs 5 fields'],
		['61 * * * *', 'Invalid minute field'],
		['0 24 * * *', 'Invalid hour field'],
		['0 0 32 * *', 'Invalid day of month field'],
		['0 0 * 13 *', 'Invalid month field'],
		['0 0 * * 8', 'Invalid day of week field'],
		['0 0 * * 5-1', 'Invalid day of week field'],
		['*/0 * * * *', 'Invalid minute field']
	])('rejects %j', (expr, message) => {
		const result = describeCron(expr);
		expect(result.ok).toBe(false);
		expect(result.text).toContain(message);
	});
});

describe('time zones', () => {
	it('lists UTC first and the browser zone is a string', () => {
		const zones = timeZones();
		expect(zones[0]).toBe('UTC');
		expect(new Set(zones).size).toBe(zones.length);
		expect(typeof browserTimeZone()).toBe('string');
	});
});
