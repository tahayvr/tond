// Access log lines (tond-proxy's format) → log viewer lines.
//
//   2026-09-25T12:00:00.123Z GET /path 200 12.3ms 1534 203.0.113.9

import type { LogLine } from '$lib/api';

const TS = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z) (.*)$/;

/** Split the leading timestamp off each line so the viewer's time column shows it. */
export function accessLogLines(lines: string[]): LogLine[] {
	return lines.map((raw) => {
		const m = TS.exec(raw);
		return m ? { source: 'container', ts: m[1], text: m[2] } : { source: 'container', text: raw };
	});
}
