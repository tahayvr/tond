import { describe, expect, it } from 'vitest';
import { color256, parseAnsi, segmentStyle, stripAnsi } from './ansi';

const ESC = '\u001b';

describe('parseAnsi', () => {
	it('returns plain text unchanged', () => {
		expect(parseAnsi('hello')).toEqual([{ text: 'hello' }]);
		expect(parseAnsi('')).toEqual([]);
	});

	it('applies and resets colours and styles', () => {
		const segs = parseAnsi(`${ESC}[1;31merror${ESC}[0m: missing ${ESC}[32mok`);
		expect(segs).toEqual([
			{ text: 'error', bold: true, fg: 'var(--ansi-1)' },
			{ text: ': missing ' },
			{ text: 'ok', fg: 'var(--ansi-2)' }
		]);
	});

	it('handles bright, 256 and truecolour codes', () => {
		expect(parseAnsi(`${ESC}[94mx`)[0].fg).toBe('var(--ansi-12)');
		expect(parseAnsi(`${ESC}[38;5;196mx`)[0].fg).toBe('rgb(255, 0, 0)');
		expect(parseAnsi(`${ESC}[48;2;1;2;3mx`)[0].bg).toBe('rgb(1, 2, 3)');
		expect(color256(244)).toBe('rgb(128, 128, 128)');
	});

	it('drops non-colour escape sequences and merges equal styles', () => {
		expect(parseAnsi(`${ESC}[2K${ESC}[1Gstep ${ESC}[1m1${ESC}[22m/3`)).toEqual([
			{ text: 'step ' },
			{ text: '1', bold: true },
			{ text: '/3' }
		]);
		expect(parseAnsi(`${ESC}]0;title\u0007done`)).toEqual([{ text: 'done' }]);
	});
});

describe('stripAnsi and segmentStyle', () => {
	it('strips every escape', () => {
		expect(stripAnsi(`${ESC}[31mred${ESC}[0m plain`)).toBe('red plain');
		expect(stripAnsi('plain')).toBe('plain');
	});

	it('builds inline CSS, swapping colours for inverse', () => {
		expect(segmentStyle({ fg: 'red', bold: true, underline: true })).toBe(
			'color:red;font-weight:700;text-decoration:underline'
		);
		expect(segmentStyle({ inverse: true, fg: 'red' })).toBe(
			'color:var(--log-bg);background-color:red'
		);
		expect(segmentStyle({})).toBe('');
	});
});
