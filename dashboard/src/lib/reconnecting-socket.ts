// WebSocket that reconnects with exponential backoff.
//
// Log streams used to die for good when tond-server restarted (an update, a
// crash, a deploy of Tond itself). connectWithRetry() reopens the socket
// after an unexpected close: 1 s, 2 s, 4 s … capped at 30 s, with jitter so a
// restarted server is not hit by every tab at once. While the tab is hidden
// no reconnect is attempted; showing the tab (or the browser coming back
// online) retries immediately. The backoff resets once a connection opens.

export interface ReconnectOptions {
	/** First retry delay (default 1000 ms). */
	minDelayMs?: number;
	/** Longest retry delay (default 30000 ms). */
	maxDelayMs?: number;
	/** Random spread added to each delay, as a fraction of it (default 0.2). */
	jitter?: number;
}

export interface ReconnectHandlers {
	/** A connection opened; `reconnected` is true after the first one. */
	onOpen?: (reconnected: boolean) => void;
	onMessage: (event: MessageEvent) => void;
	/** The connection dropped; another attempt follows in `retryInMs`. */
	onDisconnect?: (retryInMs: number) => void;
}

export interface ReconnectingSocket {
	/** Send if connected; returns whether the message was sent. */
	send: (data: string) => boolean;
	/** Close for good (no more reconnects). */
	close: () => void;
}

/** Delay before retry number `attempt` (0-based), without jitter. */
export function backoffDelay(attempt: number, minDelayMs = 1000, maxDelayMs = 30_000): number {
	return Math.min(maxDelayMs, minDelayMs * 2 ** Math.min(attempt, 30));
}

/** Resolve a path like `/api/v1/apps/x/logs/stream` to a ws:// or wss:// URL. */
export function wsUrl(path: string): string {
	const protocol = window.location.protocol === 'https:' ? 'wss' : 'ws';
	return `${protocol}://${window.location.host}${path}`;
}

export function connectWithRetry(
	url: string | (() => string),
	handlers: ReconnectHandlers,
	{ minDelayMs = 1000, maxDelayMs = 30_000, jitter = 0.2 }: ReconnectOptions = {}
): ReconnectingSocket {
	let ws: WebSocket | null = null;
	let closed = false;
	let opened = false;
	let attempt = 0;
	let timer: ReturnType<typeof setTimeout> | null = null;

	const hidden = () => typeof document !== 'undefined' && document.hidden;

	function clearTimer() {
		if (timer !== null) {
			clearTimeout(timer);
			timer = null;
		}
	}

	function open() {
		clearTimer();
		if (closed || ws) return;
		const socket = new WebSocket(typeof url === 'function' ? url() : url);
		ws = socket;
		socket.onopen = () => {
			attempt = 0;
			handlers.onOpen?.(opened);
			opened = true;
		};
		socket.onmessage = (ev) => handlers.onMessage(ev);
		socket.onclose = () => {
			if (ws !== socket) return;
			ws = null;
			if (closed) return;
			const base = backoffDelay(attempt, minDelayMs, maxDelayMs);
			const delay = Math.round(base * (1 + Math.random() * jitter));
			attempt += 1;
			handlers.onDisconnect?.(delay);
			if (!hidden()) timer = setTimeout(open, delay);
		};
		// Errors are always followed by close; reconnecting is handled there.
		socket.onerror = () => {};
	}

	function retryNow() {
		if (closed || ws || hidden()) return;
		open();
	}

	function handleVisibility() {
		if (hidden()) clearTimer();
		else retryNow();
	}

	if (typeof document !== 'undefined') {
		document.addEventListener('visibilitychange', handleVisibility);
	}
	if (typeof window !== 'undefined') {
		window.addEventListener('online', retryNow);
	}

	open();

	return {
		send(data: string) {
			if (ws && ws.readyState === WebSocket.OPEN) {
				ws.send(data);
				return true;
			}
			return false;
		},
		close() {
			closed = true;
			clearTimer();
			if (typeof document !== 'undefined') {
				document.removeEventListener('visibilitychange', handleVisibility);
			}
			if (typeof window !== 'undefined') {
				window.removeEventListener('online', retryNow);
			}
			if (ws) {
				const socket = ws;
				ws = null;
				socket.onopen = null;
				socket.onmessage = null;
				socket.onclose = null;
				socket.onerror = null;
				socket.close();
			}
		}
	};
}
