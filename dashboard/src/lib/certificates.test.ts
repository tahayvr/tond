import { describe, expect, it } from 'vitest';
import type { Certificate } from '$lib/api';
import {
	canRetry,
	certificateBadge,
	certificateProblem,
	daysLeft,
	daysLeftLabel,
	expiryTone,
	issuerLabel,
	sortCertificates,
	targetLabel
} from './certificates';

const NOW = Date.parse('2026-09-25T12:00:00Z');
const inDays = (d: number) => new Date(NOW + d * 86_400_000).toISOString();

function cert(overrides: Partial<Certificate>): Certificate {
	return {
		id: 'c1',
		domain: 'x.com',
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
		serves: { kind: 'app', app_id: 'a1', app_name: 'web' },
		...overrides
	};
}

describe('days left', () => {
	it('counts whole days and colours them', () => {
		expect(daysLeft(inDays(66.5), NOW)).toBe(66);
		expect(daysLeft(inDays(-2.5), NOW)).toBe(-3);
		expect(daysLeft('', NOW)).toBeNull();
		expect(daysLeft('garbage', NOW)).toBeNull();
		expect(expiryTone(40)).toBe('success');
		expect(expiryTone(15)).toBe('warning');
		expect(expiryTone(3)).toBe('danger');
		expect(expiryTone(null)).toBe('neutral');
	});

	it('reads naturally', () => {
		expect(daysLeftLabel(66)).toBe('66 days left');
		expect(daysLeftLabel(1)).toBe('1 day left');
		expect(daysLeftLabel(0)).toBe('expires today');
		expect(daysLeftLabel(-1)).toBe('expired 1 day ago');
		expect(daysLeftLabel(null)).toBe('—');
	});
});

describe('certificateProblem', () => {
	it('is null for a healthy certificate and for a first request in flight', () => {
		expect(certificateProblem(cert({}), NOW)).toBeNull();
		expect(certificateProblem(cert({ status: 'pending', id: '' }), NOW)).toBeNull();
	});

	it('flags failures, failing renewals, near expiry, expiry and staging', () => {
		expect(certificateProblem(cert({ status: 'failed', last_error: 'x' }), NOW)).toBe('failing');
		expect(certificateProblem(cert({ status: 'pending', last_error: 'DNS for' }), NOW)).toBe(
			'failing'
		);
		expect(certificateProblem(cert({ last_error: 'timeout' }), NOW)).toBe('renewal_failing');
		expect(certificateProblem(cert({ expires_at: inDays(12) }), NOW)).toBe('expiring');
		expect(certificateProblem(cert({ expires_at: inDays(-1) }), NOW)).toBe('expired');
		expect(certificateProblem(cert({ issuer_kind: 'letsencrypt_staging' }), NOW)).toBe('untrusted');
	});

	it('ignores the placeholder expiry of a certificate that was never issued', () => {
		expect(
			certificateProblem(cert({ status: 'failed', expires_at: inDays(-5), last_error: 'x' }), NOW)
		).toBe('failing');
	});
});

describe('badges, labels and retry', () => {
	it('labels each state', () => {
		expect(certificateBadge(cert({}), NOW)).toMatchObject({ tone: 'success', label: 'Valid' });
		expect(certificateBadge(cert({ status: 'pending' }), NOW).label).toBe('Requesting');
		expect(certificateBadge(cert({ status: 'failed', last_error: 'x' }), NOW).tone).toBe('danger');
		expect(certificateBadge(cert({ status: 'pending', last_error: 'x' }), NOW).label).toBe(
			'Waiting for DNS'
		);
	});

	it('names issuers and targets', () => {
		expect(issuerLabel('letsencrypt', "Let's Encrypt R11")).toBe("Let's Encrypt");
		expect(issuerLabel('letsencrypt_staging', null)).toBe("Let's Encrypt staging");
		expect(issuerLabel('custom', 'Pebble Intermediate CA')).toBe('Pebble Intermediate CA');
		expect(issuerLabel('unknown', null)).toBe('—');
		expect(targetLabel({ kind: 'dashboard' }).label).toBe('Tond dashboard');
		expect(targetLabel({ kind: 'app', app_id: 'a1', app_name: 'web' })).toEqual({
			label: 'web',
			href: '/apps/a1'
		});
		expect(
			targetLabel({
				kind: 'preview',
				app_id: 'a1',
				app_name: 'web',
				preview_id: 'p',
				pr_number: 4
			}).label
		).toBe('Preview of web (PR #4)');
		expect(targetLabel({ kind: 'unused' }).href).toBeNull();
	});

	it('offers retry only when something is wrong', () => {
		expect(canRetry(cert({}), NOW)).toBe(false);
		expect(canRetry(cert({ issuer_kind: 'letsencrypt_staging' }), NOW)).toBe(false);
		expect(canRetry(cert({ status: 'failed', last_error: 'x' }), NOW)).toBe(true);
		expect(canRetry(cert({ last_error: 'x' }), NOW)).toBe(true);
	});

	it('labels wildcards and uploads, and never offers ACME retries for uploads', () => {
		expect(targetLabel({ kind: 'wildcard', purpose: 'apps', hosts: 3 })).toEqual({
			label: 'Wildcard for every app (3 hosts)',
			href: '/integrations/dns'
		});
		expect(targetLabel({ kind: 'wildcard', purpose: 'previews', hosts: 1 }).label).toBe(
			'Wildcard for every preview (1 host)'
		);
		expect(issuerLabel('uploaded', 'Corp CA')).toBe('Uploaded · Corp CA');
		const upload = cert({ cert_type: 'custom', issuer_kind: 'uploaded', expires_at: inDays(5) });
		expect(certificateProblem(upload, NOW)).toBe('expiring');
		expect(canRetry(upload, NOW)).toBe(false);
	});
});

describe('sortCertificates', () => {
	it('puts problems first, then the soonest expiry', () => {
		const list = [
			cert({ domain: 'fine-late.com', expires_at: inDays(80) }),
			cert({ domain: 'renewal.com', last_error: 'x' }),
			cert({ domain: 'fine-soon.com', expires_at: inDays(40) }),
			cert({ domain: 'broken.com', status: 'failed', last_error: 'x' }),
			cert({ domain: 'gone.com', expires_at: inDays(-2) })
		];
		expect(sortCertificates(list, NOW).map((c) => c.domain)).toEqual([
			'gone.com',
			'broken.com',
			'renewal.com',
			'fine-soon.com',
			'fine-late.com'
		]);
	});
});
