import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Certificate } from '$lib/api';
import CertificateRow from './certificate-row.svelte';

const NOW = Date.parse('2026-09-25T12:00:00Z');
const inDays = (d: number) => new Date(NOW + d * 86_400_000).toISOString();

function cert(overrides: Partial<Certificate>): Certificate {
	return {
		id: 'c1',
		domain: 'shop.example.com',
		cert_type: 'acme',
		status: 'active',
		issued_at: inDays(-10),
		expires_at: inDays(80),
		last_error: null,
		last_attempt_at: null,
		next_retry_at: null,
		failure_count: 0,
		issuer: "Let's Encrypt R11",
		issuer_kind: 'letsencrypt',
		last_renewed_at: inDays(-10),
		challenge: 'http-01',
		serves: { kind: 'app', app_id: 'a1', app_name: 'shop' },
		...overrides
	};
}

describe('CertificateRow', () => {
	it('shows a healthy certificate without a retry button', () => {
		render(CertificateRow, {
			cert: cert({}),
			now: NOW,
			canWrite: true,
			retrying: false,
			onRetry: vi.fn()
		});
		expect(screen.getByText('80 days left')).toBeTruthy();
		expect(screen.getByText('Valid')).toBeTruthy();
		expect(screen.getByRole('link', { name: 'shop' }).getAttribute('href')).toBe('/apps/a1');
		expect(screen.queryByRole('button')).toBeNull();
	});

	it('explains a failing renewal and retries it', async () => {
		const onRetry = vi.fn();
		render(CertificateRow, {
			cert: cert({
				last_error: 'urn:ietf:params:acme:error:connection: Timeout during connect',
				failure_count: 3,
				next_retry_at: new Date(NOW + 30 * 60_000).toISOString()
			}),
			now: NOW,
			canWrite: true,
			retrying: false,
			onRetry
		});
		expect(screen.getByText(/couldn't connect to port 80/)).toBeTruthy();
		expect(screen.getByText(/3 failed attempts/)).toBeTruthy();
		expect(screen.getByText(/Next automatic try in 30 min/)).toBeTruthy();
		await userEvent.click(screen.getByRole('button', { name: /Retry certificate/ }));
		expect(onRetry).toHaveBeenCalledWith('shop.example.com');
	});

	it('hides retry from read-only users', () => {
		render(CertificateRow, {
			cert: cert({ status: 'failed', last_error: 'boom' }),
			now: NOW,
			canWrite: false,
			retrying: false,
			onRetry: vi.fn()
		});
		expect(screen.queryByRole('button')).toBeNull();
		expect(screen.getByText('boom')).toBeTruthy();
	});
});
