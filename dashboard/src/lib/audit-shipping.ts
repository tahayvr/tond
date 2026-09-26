// Audit log shipping: one-line delivery status and a signing-secret generator.

import type { DeliveryStatus } from '$lib/api';
import type { StatusTone } from '$lib/status';
import { formatRelativeFuture, formatRelativePast } from '$lib/time';

export interface ShippingSummary {
	tone: StatusTone;
	label: string;
	detail: string;
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;
const entries = (n: number) => plural(n, 'entry', 'entries');

/** What to say about a destination's delivery. */
export function shippingSummary(
	enabled: boolean,
	status: DeliveryStatus,
	now = Date.now()
): ShippingSummary {
	if (!enabled) {
		return { tone: 'neutral', label: 'Off', detail: 'Nothing is sent.' };
	}
	if (status.last_error) {
		const next = status.next_attempt_at
			? ` Next try ${formatRelativeFuture(status.next_attempt_at, now)}.`
			: '';
		return {
			tone: 'danger',
			label: 'Failing',
			detail: `${status.last_error}. ${entries(status.pending)} waiting after ${plural(status.failures, 'failed attempt')}.${next}`
		};
	}
	if (!status.last_success_at) {
		return {
			tone: 'progress',
			label: 'Starting',
			detail: `${entries(status.pending)} to send, starting with the oldest kept entry.`
		};
	}
	const last = `Last delivery ${formatRelativePast(status.last_success_at, now)}.`;
	const dropped =
		status.dropped > 0
			? ` ${entries(status.dropped)} expired before delivery (retention); raise the activity log retention if the destination is often down.`
			: '';
	if (status.pending > 0) {
		return {
			tone: status.dropped > 0 ? 'warning' : 'progress',
			label: 'Catching up',
			detail: `${entries(status.pending)} waiting. ${last}${dropped}`
		};
	}
	return {
		tone: status.dropped > 0 ? 'warning' : 'success',
		label: 'Up to date',
		detail: `Every entry up to #${status.shipped_seq} delivered. ${last}${dropped}`
	};
}

/** A random 32-byte hex signing secret. */
export function generateSecret(): string {
	const bytes = new Uint8Array(32);
	crypto.getRandomValues(bytes);
	return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}
