import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { contrastRatio, over, parseOklch, readTokens } from '$lib/contrast';

const css = readFileSync(resolve(__dirname, '../routes/layout.css'), 'utf8');

const themes = {
	light: readTokens(css, ':root'),
	dark: { ...readTokens(css, ':root'), ...readTokens(css, '.dark') }
};

const AA_TEXT = 4.5;
const AA_UI = 3;

describe.each(Object.entries(themes))('%s theme contrast', (_name, tokens) => {
	const color = (name: string) => parseOklch(tokens[name]);
	const surfaces = ['background', 'card', 'popover'];

	it('body and muted text meet AA on every surface', () => {
		for (const surface of surfaces) {
			expect(contrastRatio(color('foreground'), color(surface))).toBeGreaterThanOrEqual(AA_TEXT);
			expect(contrastRatio(color('muted-foreground'), color(surface))).toBeGreaterThanOrEqual(
				AA_TEXT
			);
		}
		expect(contrastRatio(color('muted-foreground'), color('muted'))).toBeGreaterThanOrEqual(
			AA_TEXT
		);
	});

	it.each(['success', 'warn', 'destructive', 'info'])(
		'%s text meets AA on surfaces and on its own 10%% tint (StatusBadge, InlineError)',
		(semantic) => {
			const fg = color(semantic);
			for (const surface of surfaces) {
				const bg = color(surface);
				expect(contrastRatio(fg, bg), `${semantic} on ${surface}`).toBeGreaterThanOrEqual(AA_TEXT);
				expect(
					contrastRatio(fg, over(fg, bg, 0.1)),
					`${semantic} on its tint over ${surface}`
				).toBeGreaterThanOrEqual(AA_TEXT);
			}
		}
	);

	it('filled buttons keep their label readable', () => {
		expect(contrastRatio(color('primary-foreground'), color('primary'))).toBeGreaterThanOrEqual(
			AA_TEXT
		);
		expect(
			contrastRatio(color('destructive-foreground'), color('destructive'))
		).toBeGreaterThanOrEqual(AA_TEXT);
	});

	it('focus ring is at least 3:1 against every surface', () => {
		for (const surface of surfaces) {
			expect(contrastRatio(color('ring'), color(surface))).toBeGreaterThanOrEqual(AA_UI);
		}
	});
});

describe('contrast maths', () => {
	it('matches known WCAG ratios', () => {
		const white = parseOklch('oklch(1 0 0)');
		const black = parseOklch('oklch(0 0 0)');
		expect(contrastRatio(white, black)).toBeCloseTo(21, 0);
		expect(contrastRatio(white, white)).toBeCloseTo(1, 5);
	});

	it('parses alpha', () => {
		expect(parseOklch('oklch(1 0 0 / 10%)').a).toBeCloseTo(0.1);
	});
});
