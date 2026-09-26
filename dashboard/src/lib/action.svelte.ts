// Pending state, error capture and live sync for every dashboard action.
//
//   const deploy = useAction(() => deployApp(id), { invalidate: true });
//   <Button onclick={() => deploy.run()} disabled={deploy.pending}>
//     {deploy.pending ? 'Deploying…' : 'Deploy'}
//   </Button>
//   <InlineError message={deploy.error} ondismiss={deploy.clear} />
//
// - `run()` is a no-op while a previous call is still pending, so a double
//   click can never start the same action twice.
// - Failures land in `error` (ApiError messages are already user-facing);
//   `run()` resolves to `undefined` instead of throwing, unless `rethrow`.
// - `invalidate: true` re-runs the page's `load` functions after success so
//   lists and detail views never show stale data (live sync).
// - `success` sets a message for a `SuccessAlert`.
// - For per-row actions pass `key` and check `isPending(rowId)`.

import { invalidateAll } from '$app/navigation';

export interface ActionOptions<A extends unknown[], R> {
	/** Re-run the page's load functions after a successful run. */
	invalidate?: boolean;
	/** Message shown in `success` after a successful run. */
	success?: string | ((result: R, ...args: A) => string);
	/** Called after success (and after invalidation). */
	onSuccess?: (result: R, ...args: A) => unknown;
	/** Re-throw failures to the caller after recording `error`. */
	rethrow?: boolean;
	/** Derive a key from the arguments so rows can show their own pending state. */
	key?: (...args: A) => string;
}

export interface Action<A extends unknown[], R> {
	readonly pending: boolean;
	readonly error: string;
	readonly success: string;
	/** Key of the in-flight call (see `ActionOptions.key`). */
	readonly pendingKey: string | null;
	isPending(key: string): boolean;
	run(...args: A): Promise<R | undefined>;
	/** Clear `error` and `success`. */
	clear(): void;
}

/** Human-readable message for anything a failed request might throw. */
export function errorMessage(e: unknown, fallback = 'Something went wrong. Try again.'): string {
	if (e instanceof Error && e.message) return e.message;
	if (typeof e === 'string' && e) return e;
	return fallback;
}

export function useAction<A extends unknown[], R>(
	fn: (...args: A) => Promise<R>,
	options: ActionOptions<A, R> = {}
): Action<A, R> {
	let pending = $state(false);
	let pendingKey = $state<string | null>(null);
	let error = $state('');
	let success = $state('');

	async function run(...args: A): Promise<R | undefined> {
		if (pending) return undefined;
		pending = true;
		pendingKey = options.key ? options.key(...args) : null;
		error = '';
		success = '';
		try {
			const result = await fn(...args);
			if (options.invalidate) await invalidateAll();
			if (options.success) {
				success =
					typeof options.success === 'function'
						? options.success(result, ...args)
						: options.success;
			}
			await options.onSuccess?.(result, ...args);
			return result;
		} catch (e) {
			error = errorMessage(e);
			if (options.rethrow) throw e;
			return undefined;
		} finally {
			pending = false;
			pendingKey = null;
		}
	}

	return {
		get pending() {
			return pending;
		},
		get error() {
			return error;
		},
		get success() {
			return success;
		},
		get pendingKey() {
			return pendingKey;
		},
		isPending(key: string) {
			return pending && pendingKey === key;
		},
		run,
		clear() {
			error = '';
			success = '';
		}
	};
}
