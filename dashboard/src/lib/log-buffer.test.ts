import { afterEach, describe, expect, it, vi } from 'vitest';

import { appendCapped, createLogBuffer } from './log-buffer';

describe('appendCapped', () => {
	it('appends while under the limit', () => {
		expect(appendCapped([1, 2], [3], 5)).toEqual([1, 2, 3]);
	});

	it('keeps only the newest entries once over the limit', () => {
		expect(appendCapped([1, 2, 3, 4], [5, 6], 4)).toEqual([3, 4, 5, 6]);
		expect(appendCapped([1], [2, 3, 4, 5, 6], 3)).toEqual([4, 5, 6]);
	});
});

describe('createLogBuffer', () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it('batches pushes into one publish per flush interval', () => {
		vi.useFakeTimers();
		const onChange = vi.fn();
		const buffer = createLogBuffer<number>(onChange, { flushMs: 100 });

		for (let i = 0; i < 50; i++) buffer.push(i);
		expect(onChange).not.toHaveBeenCalled();

		vi.advanceTimersByTime(100);
		expect(onChange).toHaveBeenCalledOnce();
		expect(onChange.mock.calls[0][0]).toHaveLength(50);
	});

	it('never holds more than the limit, however long the stream runs', () => {
		const onChange = vi.fn();
		const buffer = createLogBuffer<number>(onChange, { limit: 5_000 });

		for (let round = 0; round < 30; round++) {
			for (let i = 0; i < 1_000; i++) buffer.push(round * 1_000 + i);
			buffer.flush();
		}

		const latest = onChange.mock.lastCall?.[0] as number[];
		expect(latest).toHaveLength(5_000);
		expect(latest[0]).toBe(25_000);
		expect(latest.at(-1)).toBe(29_999);
	});

	it('replace publishes a snapshot immediately and drops queued lines', () => {
		const onChange = vi.fn();
		const buffer = createLogBuffer<string>(onChange, { limit: 2 });

		buffer.push('queued');
		buffer.replace(['a', 'b', 'c']);

		expect(onChange).toHaveBeenLastCalledWith(['b', 'c']);
		buffer.flush();
		expect(onChange).toHaveBeenCalledOnce();
	});

	it('dispose stops a pending flush', () => {
		vi.useFakeTimers();
		const onChange = vi.fn();
		const buffer = createLogBuffer<number>(onChange);

		buffer.push(1);
		buffer.dispose();
		vi.advanceTimersByTime(1_000);

		expect(onChange).not.toHaveBeenCalled();
	});
});
