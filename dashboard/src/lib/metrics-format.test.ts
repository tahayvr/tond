import { describe, expect, it } from 'vitest';
import {
	countEvents,
	formatBytes,
	formatCpu,
	formatRate,
	nearestIndex,
	niceBytesMax,
	niceMax,
	refreshInterval,
	segments,
	timeTicks
} from './metrics-format';

describe('metrics formatting', () => {
	it('formats bytes, rates and CPU', () => {
		expect(formatBytes(0)).toBe('0 B');
		expect(formatBytes(512)).toBe('512 B');
		expect(formatBytes(1536)).toBe('1.5 KB');
		expect(formatBytes(200 * 1024 * 1024)).toBe('200 MB');
		expect(formatRate(2048)).toBe('2.0 KB/s');
		expect(formatCpu(12.34)).toBe('12.3%');
		expect(formatCpu(250.4)).toBe('250%');
	});

	it('picks round axis maxima', () => {
		expect(niceMax(0)).toBe(1);
		expect(niceMax(7)).toBe(8);
		expect(niceMax(42)).toBe(50);
		expect(niceMax(130)).toBe(150);
		expect(niceMax(2.2)).toBe(2.5);
		expect(niceBytesMax(537 * 1024 * 1024)).toBe(600 * 1024 * 1024);
		expect(niceBytesMax(0)).toBe(1024);
	});
});

describe('chart geometry', () => {
	it('breaks lines at gaps longer than two buckets', () => {
		expect(segments([0, 30, 60, 180, 210], 30)).toEqual([
			[0, 1, 2],
			[3, 4]
		]);
		expect(segments([0, 30, 90], 30)).toEqual([[0, 1, 2]]);
		expect(segments([], 30)).toEqual([]);
	});

	it('finds the nearest sample', () => {
		const times = [0, 30, 60, 90];
		expect(nearestIndex(times, -10)).toBe(0);
		expect(nearestIndex(times, 44)).toBe(1);
		expect(nearestIndex(times, 46)).toBe(2);
		expect(nearestIndex(times, 500)).toBe(3);
		expect(nearestIndex([], 5)).toBe(-1);
	});

	it('spaces ticks evenly', () => {
		expect(timeTicks(0, 100, 5)).toEqual([0, 25, 50, 75, 100]);
		expect(timeTicks(10, 10)).toEqual([10]);
	});

	it('counts events and refreshes coarse ranges less often', () => {
		expect(
			countEvents([
				{ t: 1, kind: 'restart', detail: null },
				{ t: 2, kind: 'restart', detail: null },
				{ t: 3, kind: 'oom', detail: null }
			])
		).toEqual({ restarts: 2, crashes: 0, ooms: 1 });
		expect(refreshInterval('1h')).toBeLessThan(refreshInterval('30d'));
	});
});
