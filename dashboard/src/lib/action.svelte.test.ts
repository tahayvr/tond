import { beforeEach, describe, expect, it, vi } from 'vitest';

const { invalidateAll } = vi.hoisted(() => ({ invalidateAll: vi.fn() }));
vi.mock('$app/navigation', () => ({ invalidateAll }));

import { errorMessage, useAction } from './action.svelte';

describe('useAction', () => {
	beforeEach(() => invalidateAll.mockReset().mockResolvedValue(undefined));

	it('tracks pending state and ignores calls while one is in flight', async () => {
		let resolve: (v: string) => void = () => {};
		const fn = vi.fn(() => new Promise<string>((r) => (resolve = r)));
		const action = useAction(fn);

		const first = action.run();
		expect(action.pending).toBe(true);
		expect(await action.run()).toBeUndefined();
		expect(fn).toHaveBeenCalledTimes(1);

		resolve('done');
		expect(await first).toBe('done');
		expect(action.pending).toBe(false);
	});

	it('records errors instead of throwing, and clears them on the next run', async () => {
		const fn = vi.fn().mockRejectedValueOnce(new Error('Server said no')).mockResolvedValue(1);
		const action = useAction(fn);

		expect(await action.run()).toBeUndefined();
		expect(action.error).toBe('Server said no');
		await action.run();
		expect(action.error).toBe('');
	});

	it('rethrows when asked to', async () => {
		const action = useAction(() => Promise.reject(new Error('boom')), { rethrow: true });
		await expect(action.run()).rejects.toThrow('boom');
		expect(action.error).toBe('boom');
	});

	it('invalidates page data and sets a success message after success', async () => {
		const onSuccess = vi.fn();
		const action = useAction((n: number) => Promise.resolve(n * 2), {
			invalidate: true,
			success: (r) => `Got ${r}`,
			onSuccess
		});
		await action.run(21);
		expect(invalidateAll).toHaveBeenCalledOnce();
		expect(action.success).toBe('Got 42');
		expect(onSuccess).toHaveBeenCalledWith(42, 21);
		action.clear();
		expect(action.success).toBe('');
	});

	it('reports which row is pending', async () => {
		let resolve: () => void = () => {};
		const started: string[] = [];
		const action = useAction(
			(id: string) => {
				started.push(id);
				return new Promise<void>((r) => (resolve = r));
			},
			{ key: (id) => id }
		);
		const run = action.run('row-2');
		expect(started).toEqual(['row-2']);
		expect(action.isPending('row-2')).toBe(true);
		expect(action.isPending('row-1')).toBe(false);
		resolve();
		await run;
		expect(action.pendingKey).toBeNull();
	});
});

describe('errorMessage', () => {
	it('uses the error message or a fallback', () => {
		expect(errorMessage(new Error('x'))).toBe('x');
		expect(errorMessage('plain')).toBe('plain');
		expect(errorMessage(null, 'fallback')).toBe('fallback');
	});
});
