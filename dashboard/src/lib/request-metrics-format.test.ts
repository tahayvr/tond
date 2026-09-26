import { describe, expect, it } from 'vitest';
import { accessLogLines } from './access-log';
import {
	formatCount,
	formatMs,
	formatPercent,
	formatRps,
	formatTime,
	markerColor,
	maxOf,
	refreshInterval,
	sparkPath,
	stack
} from './metrics-format';
import { APP_LOG_VIEWS, CRON_TABS, allowedTabs, appLogViewFromUrl, appLogViewHref } from './tabs';

describe('request metric formatting', () => {
	it('formats rates, durations, percents and counts', () => {
		expect(formatRps(0)).toBe('0/s');
		expect(formatRps(0.123)).toBe('0.12/s');
		expect(formatRps(4.25)).toBe('4.3/s');
		expect(formatRps(42.4)).toBe('42/s');
		expect(formatRps(1234)).toBe('1.2k/s');
		expect(formatMs(null)).toBe('—');
		expect(formatMs(4)).toBe('4 ms');
		expect(formatMs(12.34)).toBe('12.3 ms');
		expect(formatMs(250.6)).toBe('251 ms');
		expect(formatMs(1500)).toBe('1.5 s');
		expect(formatPercent(null)).toBe('—');
		expect(formatPercent(0)).toBe('0%');
		expect(formatPercent(0.006)).toBe('<0.1%');
		expect(formatPercent(0.42)).toBe('0.4%');
		expect(formatPercent(12.6)).toBe('13%');
		expect(formatCount(42)).toBe('42');
		expect(formatCount(5300)).toBe('5.3k');
		expect(formatCount(1_250_000)).toBe('1.3M');
	});

	it('handles the request ranges', () => {
		expect(refreshInterval('6h')).toBe(60_000);
		expect(refreshInterval('90d')).toBe(600_000);
		// Long ranges label days, short ones times.
		expect(formatTime(0, '90d')).not.toMatch(/:/);
		expect(formatTime(0, '6h')).toMatch(/:/);
		expect(markerColor('deploy')).toBe('var(--info)');
		expect(markerColor('restart')).toBe('var(--warn)');
		expect(markerColor('oom')).toBe('var(--destructive)');
	});
});

describe('chart helpers', () => {
	it('stacks series into running totals', () => {
		expect(
			stack([
				[1, 2],
				[3, 0],
				[0, 5]
			])
		).toEqual([
			[1, 2],
			[4, 2],
			[4, 7]
		]);
		expect(stack([])).toEqual([]);
		expect(maxOf([1, null, 3], [2])).toBe(3);
		expect(maxOf([null])).toBe(0);
	});

	it('draws sparklines that break at nulls', () => {
		expect(sparkPath([], 100, 28)).toBe('');
		expect(sparkPath([0, 10], 100, 28)).toBe('M0.0,27.0L100.0,1.0');
		const broken = sparkPath([1, null, 1], 100, 28);
		expect(broken.match(/M/g)).toHaveLength(2);
		expect(sparkPath([5], 100, 28)).toBe('M50.0,1.0');
		// Values above `max` are clipped to the top.
		expect(sparkPath([20], 100, 28, 10)).toBe('M50.0,1.0');
	});
});

describe('access log lines', () => {
	it('moves the timestamp into the viewer time column', () => {
		expect(
			accessLogLines([
				'2026-09-25T12:00:00.123Z GET / 200 1.2ms 10 203.0.113.9',
				'not an access line'
			])
		).toEqual([
			{
				source: 'container',
				ts: '2026-09-25T12:00:00.123Z',
				text: 'GET / 200 1.2ms 10 203.0.113.9'
			},
			{ source: 'container', text: 'not an access line' }
		]);
	});
});

describe('log and cron tabs', () => {
	it('keeps the access log view in the URL', () => {
		expect(appLogViewFromUrl(new URL('http://x/apps/a?tab=logs'))).toBe('output');
		expect(appLogViewFromUrl(new URL('http://x/apps/a?tab=logs&log=access'))).toBe('access');
		expect(appLogViewHref('a1', 'access')).toBe('/apps/a1?tab=logs&log=access');
		expect(appLogViewHref('a1', 'output')).toBe('/apps/a1?tab=logs');
		expect(APP_LOG_VIEWS.map((v) => v.id)).toEqual(['output', 'access']);
	});

	it('hides the cron metrics tab without metrics:read', () => {
		const ids = (can: (s: string) => boolean) => allowedTabs(CRON_TABS, can).map((t) => t.id);
		expect(ids(() => true)).toContain('metrics');
		expect(ids((s) => s !== 'metrics:read')).not.toContain('metrics');
	});
});
