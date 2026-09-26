import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { backoffDelay, connectWithRetry } from './reconnecting-socket';

class FakeSocket {
	static OPEN = 1;
	static instances: FakeSocket[] = [];
	readyState = 0;
	sent: string[] = [];
	onopen: (() => void) | null = null;
	onmessage: ((ev: MessageEvent) => void) | null = null;
	onclose: (() => void) | null = null;
	onerror: (() => void) | null = null;
	closed = false;
	constructor(public url: string) {
		FakeSocket.instances.push(this);
	}
	send(data: string) {
		this.sent.push(data);
	}
	close() {
		this.closed = true;
	}
	serverOpen() {
		this.readyState = 1;
		this.onopen?.();
	}
	serverClose() {
		this.readyState = 3;
		this.onclose?.();
	}
}

function setHidden(hidden: boolean) {
	Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
	document.dispatchEvent(new Event('visibilitychange'));
}

describe('backoffDelay', () => {
	it('doubles from the minimum and caps at the maximum', () => {
		expect([0, 1, 2, 3, 4, 5, 6].map((a) => backoffDelay(a))).toEqual([
			1000, 2000, 4000, 8000, 16000, 30000, 30000
		]);
		expect(backoffDelay(100, 500, 10_000)).toBe(10_000);
	});
});

describe('connectWithRetry', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		FakeSocket.instances = [];
		vi.stubGlobal('WebSocket', FakeSocket);
		setHidden(false);
	});
	afterEach(() => {
		vi.useRealTimers();
		vi.unstubAllGlobals();
		setHidden(false);
	});

	it('reconnects with growing delays and reports reconnects', () => {
		const onOpen = vi.fn();
		const onDisconnect = vi.fn();
		const socket = connectWithRetry(
			'ws://x/stream',
			{ onOpen, onMessage: () => {}, onDisconnect },
			{ jitter: 0 }
		);
		expect(FakeSocket.instances).toHaveLength(1);
		FakeSocket.instances[0].serverOpen();
		expect(onOpen).toHaveBeenLastCalledWith(false);

		// Server restarts: first retry after 1 s, then 2 s.
		FakeSocket.instances[0].serverClose();
		expect(onDisconnect).toHaveBeenLastCalledWith(1000);
		vi.advanceTimersByTime(999);
		expect(FakeSocket.instances).toHaveLength(1);
		vi.advanceTimersByTime(1);
		expect(FakeSocket.instances).toHaveLength(2);
		FakeSocket.instances[1].serverClose();
		expect(onDisconnect).toHaveBeenLastCalledWith(2000);
		vi.advanceTimersByTime(2000);
		expect(FakeSocket.instances).toHaveLength(3);

		// Back up: backoff resets.
		FakeSocket.instances[2].serverOpen();
		expect(onOpen).toHaveBeenLastCalledWith(true);
		expect(socket.send('hi')).toBe(true);
		FakeSocket.instances[2].serverClose();
		expect(onDisconnect).toHaveBeenLastCalledWith(1000);
		socket.close();
	});

	it('does not retry while the tab is hidden and retries when shown', () => {
		connectWithRetry('ws://x', { onMessage: () => {} }, { jitter: 0 });
		FakeSocket.instances[0].serverOpen();
		setHidden(true);
		FakeSocket.instances[0].serverClose();
		vi.advanceTimersByTime(60_000);
		expect(FakeSocket.instances).toHaveLength(1);
		setHidden(false);
		expect(FakeSocket.instances).toHaveLength(2);
	});

	it('stops for good when closed', () => {
		const socket = connectWithRetry('ws://x', { onMessage: () => {} });
		const first = FakeSocket.instances[0];
		socket.close();
		expect(first.closed).toBe(true);
		first.serverClose();
		vi.advanceTimersByTime(60_000);
		expect(FakeSocket.instances).toHaveLength(1);
		expect(socket.send('x')).toBe(false);
	});

	it('forwards messages', () => {
		const onMessage = vi.fn();
		connectWithRetry(() => 'ws://computed', { onMessage });
		expect(FakeSocket.instances[0].url).toBe('ws://computed');
		FakeSocket.instances[0].onmessage?.({ data: 'x' } as MessageEvent);
		expect(onMessage).toHaveBeenCalledTimes(1);
	});
});
