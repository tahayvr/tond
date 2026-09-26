import { describe, expect, it } from 'vitest';
import type { DeliveryStatus } from '$lib/api';
import { generateSecret, shippingSummary } from './audit-shipping';

const NOW = Date.parse('2026-09-25T12:00:00Z');
const ago = (min: number) => new Date(NOW - min * 60_000).toISOString();

function status(overrides: Partial<DeliveryStatus>): DeliveryStatus {
	return {
		shipped_seq: 10,
		pending: 0,
		failures: 0,
		last_attempt_at: ago(4),
		last_success_at: ago(4),
		last_error: null,
		next_attempt_at: null,
		dropped: 0,
		...overrides
	};
}

describe('shippingSummary', () => {
	it('says off, up to date, catching up and starting', () => {
		expect(shippingSummary(false, status({}), NOW).label).toBe('Off');
		const ok = shippingSummary(true, status({}), NOW);
		expect(ok).toMatchObject({ tone: 'success', label: 'Up to date' });
		expect(ok.detail).toContain('#10');
		expect(ok.detail).toContain('4m ago');
		expect(shippingSummary(true, status({ pending: 3 }), NOW).label).toBe('Catching up');
		expect(
			shippingSummary(true, status({ last_success_at: null, pending: 1 }), NOW).detail
		).toContain('1 entry to send');
	});

	it('explains failures with the backoff', () => {
		const s = shippingSummary(
			true,
			status({
				last_error: 'TLS handshake failed',
				failures: 3,
				pending: 5,
				next_attempt_at: new Date(NOW + 20 * 60_000).toISOString()
			}),
			NOW
		);
		expect(s.tone).toBe('danger');
		expect(s.detail).toBe(
			'TLS handshake failed. 5 entries waiting after 3 failed attempts. Next try in 20m.'
		);
	});

	it('warns about entries lost to retention', () => {
		const s = shippingSummary(true, status({ dropped: 2 }), NOW);
		expect(s.tone).toBe('warning');
		expect(s.detail).toContain('2 entries expired before delivery');
	});
});

describe('generateSecret', () => {
	it('makes 64 hex characters, different each time', () => {
		const a = generateSecret();
		expect(a).toMatch(/^[0-9a-f]{64}$/);
		expect(generateSecret()).not.toBe(a);
	});
});
