// WCAG contrast maths for the theme tokens in routes/layout.css.
// Used by the contrast test to keep every semantic colour at AA in both themes.

export interface Rgba {
	r: number;
	g: number;
	b: number;
	a: number;
}

/** Parse `oklch(L C H)` or `oklch(L C H / A%)` into linear-free sRGB (0..1, clamped). */
export function parseOklch(value: string): Rgba {
	const m = value
		.trim()
		.match(/^oklch\(\s*([\d.]+)(%?)\s+([\d.]+)\s+([\d.]+)\s*(?:\/\s*([\d.]+)(%?))?\s*\)$/);
	if (!m) throw new Error(`not an oklch() colour: ${value}`);
	const l = parseFloat(m[1]) / (m[2] ? 100 : 1);
	const c = parseFloat(m[3]);
	const h = (parseFloat(m[4]) * Math.PI) / 180;
	const alpha = m[5] === undefined ? 1 : parseFloat(m[5]) / (m[6] ? 100 : 1);
	return { ...oklabToSrgb(l, c * Math.cos(h), c * Math.sin(h)), a: alpha };
}

function oklabToSrgb(L: number, a: number, b: number): Omit<Rgba, 'a'> {
	const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
	const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
	const s_ = L - 0.0894841775 * a - 1.291485548 * b;
	const l = l_ ** 3;
	const m = m_ ** 3;
	const s = s_ ** 3;
	const lin = [
		4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
		-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
		-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s
	];
	const [r, g, bl] = lin.map((v) => {
		const x = Math.min(1, Math.max(0, v));
		return x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055;
	});
	return { r, g, b: bl };
}

/** Composite `top` (with alpha) over an opaque `bottom`. */
export function over(top: Rgba, bottom: Rgba, alpha = top.a): Rgba {
	return {
		r: top.r * alpha + bottom.r * (1 - alpha),
		g: top.g * alpha + bottom.g * (1 - alpha),
		b: top.b * alpha + bottom.b * (1 - alpha),
		a: 1
	};
}

function luminance({ r, g, b }: Rgba): number {
	const f = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
	return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
}

export function contrastRatio(a: Rgba, b: Rgba): number {
	const la = luminance(a);
	const lb = luminance(b);
	return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Extract `--name: value;` declarations from a CSS block body. */
export function readTokens(css: string, selector: string): Record<string, string> {
	const start = css.indexOf(`${selector} {`);
	if (start < 0) throw new Error(`selector ${selector} not found`);
	const body = css.slice(start, css.indexOf('}', start));
	const tokens: Record<string, string> = {};
	for (const m of body.matchAll(/--([\w-]+):\s*([^;]+);/g)) tokens[m[1]] = m[2].trim();
	return tokens;
}
