import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { pollWhileVisible } from './polling';

let hidden = false;

function setHidden(value: boolean) {
	hidden = value;
	document.dispatchEvent(new Event('visibilitychange'));
}

describe('pollWhileVisible', () => {
	beforeEach(() => {
		vi.useFakeTimers();
		hidden = false;
		Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it('polls immediately and then on every interval', async () => {
		const fn = vi.fn().mockResolvedValue(undefined);
		const stop = pollWhileVisible(fn, 1_000);

		await vi.advanceTimersByTimeAsync(0);
		expect(fn).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(3_000);
		expect(fn).toHaveBeenCalledTimes(4);

		stop();
	});

	it('pauses while the tab is hidden and catches up when it is shown again', async () => {
		const fn = vi.fn().mockResolvedValue(undefined);
		const stop = pollWhileVisible(fn, 1_000);
		await vi.advanceTimersByTimeAsync(0);

		setHidden(true);
		await vi.advanceTimersByTimeAsync(10_000);
		expect(fn).toHaveBeenCalledTimes(1);

		setHidden(false);
		await vi.advanceTimersByTimeAsync(0);
		expect(fn).toHaveBeenCalledTimes(2);
		await vi.advanceTimersByTimeAsync(1_000);
		expect(fn).toHaveBeenCalledTimes(3);

		stop();
	});

	it('keeps polling after a failed call and stops for good when stopped', async () => {
		const fn = vi.fn().mockRejectedValue(new Error('boom'));
		const stop = pollWhileVisible(fn, 1_000);
		await vi.advanceTimersByTimeAsync(2_000);
		expect(fn).toHaveBeenCalledTimes(3);

		stop();
		setHidden(true);
		setHidden(false);
		await vi.advanceTimersByTimeAsync(5_000);
		expect(fn).toHaveBeenCalledTimes(3);
	});
});
