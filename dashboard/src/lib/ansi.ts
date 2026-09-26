// ANSI SGR (colour/style) parsing for log lines.
//
// Build tools, Docker and most runtimes colour their output with SGR escape
// sequences (`ESC[31m` … `ESC[0m`). parseAnsi() turns one line into styled
// segments; every other escape sequence (cursor movement, erase line, OSC
// titles) is dropped. The 16 basic colours map to CSS variables
// (`--ansi-0` … `--ansi-15`, defined per theme in layout.css) so they stay
// readable in light and dark mode; 256-colour and truecolour codes are
// passed through as literal colours.

export interface AnsiStyle {
	/** CSS colour value */
	fg?: string;
	bg?: string;
	bold?: boolean;
	dim?: boolean;
	italic?: boolean;
	underline?: boolean;
	inverse?: boolean;
}

export interface AnsiSegment extends AnsiStyle {
	text: string;
}

// eslint-disable-next-line no-control-regex
const ESCAPE_RE = /\u001b(?:\[([0-9;:?]*)([@-~])|\][^\u0007\u001b]*(?:\u0007|\u001b\\)|[@-Z\\-_])/g;

/** CSS colour for a 16-colour palette index. */
function paletteColor(index: number): string {
	return `var(--ansi-${index})`;
}

/** xterm 256-colour palette → CSS colour. */
export function color256(n: number): string {
	if (n < 16) return paletteColor(n);
	if (n < 232) {
		const i = n - 16;
		const steps = [0, 95, 135, 175, 215, 255];
		const r = steps[Math.floor(i / 36) % 6];
		const g = steps[Math.floor(i / 6) % 6];
		const b = steps[i % 6];
		return `rgb(${r}, ${g}, ${b})`;
	}
	const level = 8 + (n - 232) * 10;
	return `rgb(${level}, ${level}, ${level})`;
}

function applySgr(style: AnsiStyle, params: string): AnsiStyle {
	const codes = params === '' ? [0] : params.split(/[;:]/).map((p) => (p === '' ? 0 : Number(p)));
	const next: AnsiStyle = { ...style };
	for (let i = 0; i < codes.length; i++) {
		const code = codes[i];
		if (code === 0) {
			for (const key of Object.keys(next) as (keyof AnsiStyle)[]) delete next[key];
		} else if (code === 1) next.bold = true;
		else if (code === 2) next.dim = true;
		else if (code === 3) next.italic = true;
		else if (code === 4) next.underline = true;
		else if (code === 7) next.inverse = true;
		else if (code === 22) {
			delete next.bold;
			delete next.dim;
		} else if (code === 23) delete next.italic;
		else if (code === 24) delete next.underline;
		else if (code === 27) delete next.inverse;
		else if (code >= 30 && code <= 37) next.fg = paletteColor(code - 30);
		else if (code === 39) delete next.fg;
		else if (code >= 40 && code <= 47) next.bg = paletteColor(code - 40);
		else if (code === 49) delete next.bg;
		else if (code >= 90 && code <= 97) next.fg = paletteColor(code - 90 + 8);
		else if (code >= 100 && code <= 107) next.bg = paletteColor(code - 100 + 8);
		else if (code === 38 || code === 48) {
			const target = code === 38 ? 'fg' : 'bg';
			if (codes[i + 1] === 5 && codes[i + 2] !== undefined) {
				next[target] = color256(codes[i + 2]);
				i += 2;
			} else if (codes[i + 1] === 2 && codes[i + 4] !== undefined) {
				next[target] = `rgb(${codes[i + 2]}, ${codes[i + 3]}, ${codes[i + 4]})`;
				i += 4;
			}
		}
	}
	return next;
}

function sameStyle(a: AnsiStyle, b: AnsiStyle): boolean {
	return (
		a.fg === b.fg &&
		a.bg === b.bg &&
		!!a.bold === !!b.bold &&
		!!a.dim === !!b.dim &&
		!!a.italic === !!b.italic &&
		!!a.underline === !!b.underline &&
		!!a.inverse === !!b.inverse
	);
}

/** Split one log line into styled segments (escape sequences removed). */
export function parseAnsi(line: string): AnsiSegment[] {
	if (!line.includes('\u001b')) return line ? [{ text: line }] : [];
	const segments: AnsiSegment[] = [];
	let style: AnsiStyle = {};
	let last = 0;
	const push = (text: string) => {
		if (!text) return;
		const prev = segments[segments.length - 1];
		if (prev && sameStyle(prev, style)) prev.text += text;
		else segments.push({ ...style, text });
	};
	ESCAPE_RE.lastIndex = 0;
	let m: RegExpExecArray | null;
	while ((m = ESCAPE_RE.exec(line)) !== null) {
		push(line.slice(last, m.index));
		last = m.index + m[0].length;
		if (m[2] === 'm') style = applySgr(style, m[1] ?? '');
	}
	push(line.slice(last));
	return segments;
}

/** The line without any escape sequences. */
export function stripAnsi(line: string): string {
	if (!line.includes('\u001b')) return line;
	return line.replace(ESCAPE_RE, '');
}

/** Inline CSS for a segment's style. */
export function segmentStyle(s: AnsiStyle): string {
	const fg = s.inverse ? (s.bg ?? 'var(--log-bg)') : s.fg;
	const bg = s.inverse ? (s.fg ?? 'var(--log-fg)') : s.bg;
	const parts: string[] = [];
	if (fg) parts.push(`color:${fg}`);
	if (bg) parts.push(`background-color:${bg}`);
	if (s.bold) parts.push('font-weight:700');
	if (s.dim) parts.push('opacity:0.7');
	if (s.italic) parts.push('font-style:italic');
	if (s.underline) parts.push('text-decoration:underline');
	return parts.join(';');
}
