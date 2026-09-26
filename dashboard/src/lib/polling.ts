// Interval polling that pauses while the browser tab is hidden.
//
// Background tabs kept hitting the API every few seconds for data nobody was
// looking at. pollWhileVisible() stops the timer when `document.hidden` becomes
// true and, when the tab is shown again, polls once right away and resumes.

export interface PollOptions {
	/** Run `fn` immediately when polling starts (default true). */
	immediate?: boolean;
}

/**
 * Call `fn` every `intervalMs` while the page is visible. Calls never overlap:
 * the next one is scheduled after the previous one settles. Returns a stop
 * function (use it as an `$effect` cleanup).
 */
export function pollWhileVisible(
	fn: () => unknown,
	intervalMs: number,
	{ immediate = true }: PollOptions = {}
): () => void {
	let timer: ReturnType<typeof setTimeout> | null = null;
	let stopped = false;
	let running = false;

	const hidden = () => typeof document !== 'undefined' && document.hidden;

	function clear() {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	function schedule() {
		clear();
		if (stopped || hidden()) return;
		timer = setTimeout(tick, intervalMs);
	}

	async function tick() {
		timer = null;
		if (stopped || running || hidden()) return;
		running = true;
		try {
			await fn();
		} catch {
			// Callers handle their own errors; a failed poll must not stop polling.
		} finally {
			running = false;
			schedule();
		}
	}

	function handleVisibility() {
		if (hidden()) {
			clear();
		} else if (!running) {
			tick();
		}
	}

	if (typeof document !== 'undefined') {
		document.addEventListener('visibilitychange', handleVisibility);
	}

	if (immediate) {
		tick();
	} else {
		schedule();
	}

	return () => {
		stopped = true;
		clear();
		if (typeof document !== 'undefined') {
			document.removeEventListener('visibilitychange', handleVisibility);
		}
	};
}
