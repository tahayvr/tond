import { render, screen } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Certificate } from '$lib/api';
import CertificateStatus, {
	certificateView,
	explainCertError,
	formatRetryIn
} from './certificate-status.svelte';

const api = vi.hoisted(() => ({
	listCertificates: vi.fn(),
	retryCertificate: vi.fn()
}));

vi.mock('$lib/api', async () => {
	const actual = await vi.importActual<typeof import('$lib/api')>('$lib/api');
	return { ...actual, ...api };
});

const NOW = Date.parse('2026-09-25T12:00:00Z');

function cert(overrides: Partial<Certificate>): Certificate {
	return {
		id: 'c1',
		domain: 'x.com',
		cert_type: 'acme',
		status: 'pending',
		issued_at: '2026-09-25T00:00:00Z',
		expires_at: '2026-12-24T00:00:00Z',
		last_error: null,
		last_attempt_at: null,
		next_retry_at: null,
		failure_count: 0,
		issuer: null,
		issuer_kind: 'unknown',
		last_renewed_at: null,
		challenge: null,
		serves: { kind: 'unused' },
		...overrides
	};
}

describe('formatRetryIn', () => {
	it('formats minutes and hours until the next attempt', () => {
		expect(formatRetryIn(null, NOW)).toBeNull();
		expect(formatRetryIn('2026-09-25T12:00:30Z', NOW)).toBe('less than a minute');
		expect(formatRetryIn('2026-09-25T12:12:00Z', NOW)).toBe('12 min');
		expect(formatRetryIn('2026-09-25T15:00:00Z', NOW)).toBe('3 h');
		expect(formatRetryIn('2026-09-25T13:30:00Z', NOW)).toBe('1 h 30 min');
	});
});

describe('certificateView', () => {
	it('distinguishes missing, requesting, retrying and active certificates', () => {
		expect(certificateView(undefined, NOW)).toEqual({ kind: 'none' });
		expect(certificateView(cert({}), NOW)).toEqual({ kind: 'requesting' });
		expect(
			certificateView(
				cert({
					status: 'pending',
					last_error: 'DNS for x.com points to 1.2.3.4, expected 5.6.7.8.',
					next_retry_at: '2026-09-25T12:12:00Z'
				}),
				NOW
			)
		).toEqual({
			kind: 'retrying',
			failed: false,
			error: 'DNS for x.com points to 1.2.3.4, expected 5.6.7.8.',
			retryIn: '12 min'
		});
		expect(certificateView(cert({ status: 'active' }), NOW)).toMatchObject({
			kind: 'active',
			renewalError: null
		});
	});
});

describe('CertificateStatus', () => {
	beforeEach(() => {
		api.listCertificates.mockReset();
		api.retryCertificate.mockReset();
	});

	it('shows the last error with the retry time and lets the user retry now', async () => {
		const pending = cert({
			domain: 'app.example.com',
			last_error: 'DNS for app.example.com points to 1.2.3.4, expected 5.6.7.8.',
			next_retry_at: new Date(Date.now() + 12 * 60_000).toISOString(),
			failure_count: 4
		});
		api.listCertificates.mockResolvedValue([pending]);
		api.retryCertificate.mockResolvedValue(undefined);

		render(CertificateStatus, { domain: 'app.example.com' });

		expect(await screen.findByText(/points to 1\.2\.3\.4, expected 5\.6\.7\.8/)).toBeTruthy();
		expect(screen.getByText(/Certificate pending for app\.example\.com/)).toBeTruthy();
		expect(screen.getByText(/retries automatically in 1[12] min/)).toBeTruthy();

		await userEvent.click(screen.getByRole('button', { name: /Retry now/ }));
		expect(api.retryCertificate).toHaveBeenCalledWith('app.example.com');
	});

	it('renders nothing without a domain', () => {
		const { container } = render(CertificateStatus, { domain: '' });
		expect(container.querySelector('[data-testid="certificate-status"]')).toBeNull();
		expect(api.listCertificates).not.toHaveBeenCalled();
	});
});

describe('explainCertError', () => {
	const d = 'app.example.com';

	it('keeps messages that are already readable', () => {
		expect(
			explainCertError('DNS for app.example.com points to 1.2.3.4, expected 5.6.7.8', d)
		).toBeNull();
		expect(explainCertError('something unexpected', d)).toBeNull();
	});

	it('explains the common Let’s Encrypt failures', () => {
		const cases: [string, string][] = [
			[
				'order failed: 403 urn:ietf:params:acme:error:unauthorized: Invalid response from http://app.example.com/.well-known/acme-challenge/abc: 404',
				"wasn't this one"
			],
			[
				'urn:ietf:params:acme:error:connection: Timeout during connect (likely firewall problem)',
				'port 80'
			],
			[
				'urn:ietf:params:acme:error:dns: DNS problem: NXDOMAIN looking up A for app.example.com',
				"doesn't resolve"
			],
			[
				'urn:ietf:params:acme:error:rateLimited: too many failed authorizations recently',
				'rate limit'
			],
			[
				'urn:ietf:params:acme:error:caa: CAA record for example.com prevents issuance',
				'CAA record'
			],
			['urn:ietf:params:acme:error:rejectedIdentifier: Error creating new order', "won't issue"]
		];
		for (const [raw, expected] of cases) {
			expect(explainCertError(raw, d)).toContain(expected);
		}
	});
});
