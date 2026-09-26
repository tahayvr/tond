import { describe, expect, it } from 'vitest';
import {
	collapseCarriageReturns,
	findMatches,
	highlightSegments,
	logFileName,
	prepareLine,
	toDownloadText,
	visibleRange
} from './log-view';

const ESC = '\u001b';

describe('prepareLine', () => {
	it('parses colours, keeps plain text and level for Tond lines', () => {
		const p = prepareLine(
			{ source: 'deploy', level: 'error', text: `${ESC}[31mBuild failed${ESC}[0m`, ts: 't' },
			3
		);
		expect(p.index).toBe(3);
		expect(p.plain).toBe('Build failed');
		expect(p.haystack).toBe('build failed');
		expect(p.level).toBe('error');
		expect(p.segments[0].fg).toBe('var(--ansi-1)');
	});

	it('marks program output without a level and prefixes app names', () => {
		const p = prepareLine({ source: 'container', text: 'GET /', appName: 'web' }, 0);
		expect(p.level).toBeUndefined();
		expect(p.prefix).toBe('web');
		expect(p.haystack).toBe('web get /');
		expect(prepareLine({ source: 'container', text: 'x', appName: 'web' }, 0, false).prefix).toBe(
			undefined
		);
	});

	it('collapses carriage-return progress', () => {
		expect(collapseCarriageReturns('10%\r50%\r100%')).toBe('100%');
		expect(collapseCarriageReturns('line\r')).toBe('line');
	});
});

describe('search', () => {
	const lines = ['alpha', 'Beta error', 'gamma', 'ERROR again'].map((text, i) =>
		prepareLine({ source: 'container', text }, i)
	);

	it('finds matches case-insensitively', () => {
		expect(findMatches(lines, 'error')).toEqual([1, 3]);
		expect(findMatches(lines, '  ')).toEqual([]);
	});

	it('highlights across segment boundaries', () => {
		const segs = highlightSegments([{ text: 'foo ba', bold: true }, { text: 'r baz' }], 'bar');
		expect(segs).toEqual([
			{ text: 'foo ', bold: true },
			{ text: 'ba', bold: true, match: true },
			{ text: 'r', match: true },
			{ text: ' baz' }
		]);
		expect(highlightSegments([{ text: 'aaa' }], 'a').filter((s) => 'match' in s)).toHaveLength(3);
		expect(highlightSegments([{ text: 'x' }], 'y')).toEqual([{ text: 'x' }]);
	});
});

describe('download and virtualization', () => {
	it('builds the download text', () => {
		const lines = [
			prepareLine(
				{ source: 'deploy', level: 'warn', text: 'careful', ts: '2026-01-01T00:00:00Z' },
				0
			),
			prepareLine({ source: 'container', text: `${ESC}[32mok`, appName: 'api' }, 1)
		];
		expect(toDownloadText(lines, true)).toBe('2026-01-01T00:00:00Z [warn] careful\n[api] ok\n');
		expect(toDownloadText(lines, false)).toBe('[warn] careful\n[api] ok\n');
		expect(toDownloadText([], true)).toBe('');
	});

	it('computes the rendered window', () => {
		expect(visibleRange(0, 400, 20, 1000, 10)).toEqual({ start: 0, end: 30 });
		expect(visibleRange(2000, 400, 20, 1000, 10)).toEqual({ start: 90, end: 130 });
		expect(visibleRange(19_990, 400, 20, 1000, 10)).toEqual({ start: 989, end: 1000 });
		expect(visibleRange(0, 400, 20, 0)).toEqual({ start: 0, end: 0 });
	});

	it('names files safely', () => {
		expect(logFileName('deploy web/app', new Date('2026-09-25T14:03:07Z'))).toBe(
			'deploy-web-app-2026-09-25T14-03.log'
		);
		expect(logFileName('///', new Date('2026-09-25T14:03:07Z'))).toBe('logs-2026-09-25T14-03.log');
	});
});
