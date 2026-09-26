// Bounded, batched log buffer for live log views.
//
// Appending one line at a time with `lines = [...lines, line]` copies the whole
// array per line (O(n²) over a stream) and never forgets old lines. This buffer
// collects incoming lines and publishes them at most once per `flushMs`, keeping
// only the newest `limit` lines — one bounded copy per flush.

export const LOG_BUFFER_LIMIT = 5_000;

/** `current` followed by `incoming`, trimmed to the newest `limit` entries. */
export function appendCapped<T>(current: readonly T[], incoming: readonly T[], limit: number): T[] {
	if (incoming.length >= limit) return incoming.slice(incoming.length - limit);
	const overflow = current.length + incoming.length - limit;
	const kept = overflow > 0 ? current.slice(overflow) : current;
	return kept.concat(incoming);
}

export interface LogBuffer<T> {
	/** Queue one line; it is published on the next flush. */
	push(line: T): void;
	/** Replace everything (e.g. with a snapshot frame). Publishes immediately. */
	replace(lines: readonly T[]): void;
	clear(): void;
	/** Publish queued lines now. */
	flush(): void;
	/** Drop queued lines and stop the flush timer. */
	dispose(): void;
}

export function createLogBuffer<T>(
	onChange: (lines: T[]) => void,
	{ limit = LOG_BUFFER_LIMIT, flushMs = 100 }: { limit?: number; flushMs?: number } = {}
): LogBuffer<T> {
	let lines: T[] = [];
	let pending: T[] = [];
	let timer: ReturnType<typeof setTimeout> | null = null;

	function cancelTimer() {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	function flush() {
		cancelTimer();
		if (pending.length === 0) return;
		lines = appendCapped(lines, pending, limit);
		pending = [];
		onChange(lines);
	}

	return {
		push(line) {
			pending.push(line);
			if (timer === null) timer = setTimeout(flush, flushMs);
		},
		replace(next) {
			cancelTimer();
			pending = [];
			lines = appendCapped([], next, limit);
			onChange(lines);
		},
		clear() {
			cancelTimer();
			pending = [];
			lines = [];
			onChange(lines);
		},
		flush,
		dispose() {
			cancelTimer();
			pending = [];
		}
	};
}
