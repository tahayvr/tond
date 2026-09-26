// Pure helpers behind the log viewer: prepare lines once (ANSI parsed,
// carriage-return progress collapsed, plain text for search), find search
// matches, split segments around highlights, format timestamps and build the
// download text. Kept free of Svelte so every rule is unit-tested.

import { parseAnsi, stripAnsi, type AnsiSegment } from '$lib/ansi';
import type { LogLine } from '$lib/api';

export interface PreparedLine {
	/** Stable index in the source array */
	index: number;
	segments: AnsiSegment[];
	/** Text without escape codes (search and download) */
	plain: string;
	/** Lowercased plain text, prefix included, for case-insensitive search */
	haystack: string;
	ts?: string;
	/** "error" | "warn" | "info" for Tond's own lines; undefined for program output */
	level?: string;
	/** Short label shown before the text, e.g. an app name or "[deploy]" */
	prefix?: string;
}

/**
 * A line that redraws itself with `\r` (progress bars) shows only what was
 * drawn last; a trailing `\r` (CRLF output) is ignored.
 */
export function collapseCarriageReturns(text: string): string {
	const trimmed = text.replace(/\r+$/, '');
	const at = trimmed.lastIndexOf('\r');
	return at === -1 ? trimmed : trimmed.slice(at + 1);
}

export function prepareLine(line: LogLine, index: number, showPrefix = true): PreparedLine {
	const text = collapseCarriageReturns(line.text ?? '');
	const plain = stripAnsi(text);
	// Multi-app streams tag lines with the app, compose apps with the service.
	const source =
		line.appName && line.service
			? `${line.appName}/${line.service}`
			: (line.appName ?? line.service);
	const prefix = showPrefix && source ? source : undefined;
	return {
		index,
		segments: parseAnsi(text),
		plain,
		haystack: (prefix ? `${prefix} ${plain}` : plain).toLowerCase(),
		ts: line.ts,
		level: line.source === 'deploy' ? (line.level ?? 'info') : undefined,
		prefix
	};
}

/** Indices (into `lines`) of lines containing `query`, case-insensitively. */
export function findMatches(lines: readonly PreparedLine[], query: string): number[] {
	const q = query.trim().toLowerCase();
	if (!q) return [];
	const out: number[] = [];
	for (let i = 0; i < lines.length; i++) {
		if (lines[i].haystack.includes(q)) out.push(i);
	}
	return out;
}

export interface HighlightedSegment extends AnsiSegment {
	match?: boolean;
}

/** Split segments so every occurrence of `query` is its own `match` segment. */
export function highlightSegments(segments: AnsiSegment[], query: string): HighlightedSegment[] {
	const q = query.trim().toLowerCase();
	if (!q) return segments;
	const out: HighlightedSegment[] = [];
	// Search across segment boundaries on the joined text, then cut.
	const full = segments.map((s) => s.text).join('');
	const lower = full.toLowerCase();
	const ranges: [number, number][] = [];
	let from = 0;
	while (from <= lower.length) {
		const at = lower.indexOf(q, from);
		if (at === -1) break;
		ranges.push([at, at + q.length]);
		from = at + q.length;
	}
	if (ranges.length === 0) return segments;
	let pos = 0;
	for (const seg of segments) {
		const start = pos;
		const end = pos + seg.text.length;
		let cursor = start;
		for (const [rs, re] of ranges) {
			if (re <= cursor || rs >= end) continue;
			const a = Math.max(rs, cursor);
			const b = Math.min(re, end);
			if (a > cursor) out.push({ ...seg, text: full.slice(cursor, a) });
			out.push({ ...seg, text: full.slice(a, b), match: true });
			cursor = b;
		}
		if (cursor < end) out.push({ ...seg, text: full.slice(cursor, end) });
		pos = end;
	}
	return out;
}

/** "14:03:07" (24-hour, fixed width) in the viewer's locale; "" when missing or unparseable. */
export function formatTime(ts: string | undefined): string {
	if (!ts) return '';
	const d = new Date(ts);
	if (Number.isNaN(d.getTime())) return '';
	return d.toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit',
		hourCycle: 'h23'
	});
}

/** The text a download contains: one line per entry, optional ISO timestamps. */
export function toDownloadText(lines: readonly PreparedLine[], withTimestamps: boolean): string {
	return (
		lines
			.map((l) => {
				const parts: string[] = [];
				if (withTimestamps && l.ts) parts.push(l.ts);
				if (l.prefix) parts.push(`[${l.prefix}]`);
				if (l.level && l.level !== 'info') parts.push(`[${l.level}]`);
				parts.push(l.plain);
				return parts.join(' ');
			})
			.join('\n') + (lines.length ? '\n' : '')
	);
}

/** Window of rows to render for a virtualized list. */
export function visibleRange(
	scrollTop: number,
	viewportHeight: number,
	rowHeight: number,
	total: number,
	overscan = 20
): { start: number; end: number } {
	if (total === 0 || rowHeight <= 0) return { start: 0, end: 0 };
	const first = Math.floor(scrollTop / rowHeight);
	const count = Math.ceil(viewportHeight / rowHeight);
	const start = Math.max(0, first - overscan);
	const end = Math.min(total, first + count + overscan);
	return { start, end };
}

/** A safe file name: "deploy-web-2026-09-25T14-03.log". */
export function logFileName(base: string, now: Date = new Date()): string {
	const stamp = now.toISOString().slice(0, 16).replace(/:/g, '-');
	const clean = base.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'logs';
	return `${clean}-${stamp}.log`;
}
