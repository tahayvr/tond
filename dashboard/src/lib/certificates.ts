// Certificates page logic: days left, what's wrong, labels and ordering.
//
// tond-proxy renews a certificate when it has 30 days left, so an active
// certificate with less than RENEWAL_OVERDUE_DAYS left means renewals have
// been failing for a while.

import type { Certificate, CertificateTarget, IssuerKind } from '$lib/api';
import type { StatusTone } from '$lib/status';

const DAY_MS = 86_400_000;
/** Below this many days left, renewal is overdue (warning colour). */
export const RENEWAL_OVERDUE_DAYS = 21;
/** Below this many days left, the certificate is about to lapse (danger colour). */
export const EXPIRY_DANGER_DAYS = 7;

/** Whole days until `expiresAt` (negative once expired), or null if unknown. */
export function daysLeft(expiresAt: string | null | undefined, now: number): number | null {
	if (!expiresAt) return null;
	const t = Date.parse(expiresAt);
	if (Number.isNaN(t)) return null;
	return Math.floor((t - now) / DAY_MS);
}

/** Colour for a days-left figure. */
export function expiryTone(days: number | null): StatusTone {
	if (days === null) return 'neutral';
	if (days < EXPIRY_DANGER_DAYS) return 'danger';
	if (days < RENEWAL_OVERDUE_DAYS) return 'warning';
	return 'success';
}

/** "66 days left", "1 day left", "expires today", "expired 3 days ago". */
export function daysLeftLabel(days: number | null): string {
	if (days === null) return '—';
	if (days < 0) {
		const ago = -days;
		return `expired ${ago} ${ago === 1 ? 'day' : 'days'} ago`;
	}
	if (days === 0) return 'expires today';
	return `${days} ${days === 1 ? 'day' : 'days'} left`;
}

/** Is there a usable (issued) certificate behind this row? */
export function hasIssuedCertificate(cert: Certificate): boolean {
	return cert.status === 'active' || cert.status === 'expired';
}

export type CertificateProblem =
	'failing' | 'renewal_failing' | 'expiring' | 'expired' | 'untrusted' | null;

/** What needs attention on this certificate, most serious first; null if fine. */
export function certificateProblem(cert: Certificate, now: number): CertificateProblem {
	const days = hasIssuedCertificate(cert) ? daysLeft(cert.expires_at, now) : null;
	if (cert.status === 'expired' || (days !== null && days < 0)) return 'expired';
	if (cert.status === 'failed') return 'failing';
	if (cert.status === 'pending') return cert.last_error ? 'failing' : null;
	if (days !== null && days < RENEWAL_OVERDUE_DAYS) return 'expiring';
	if (cert.last_error) return 'renewal_failing';
	if (cert.issuer_kind === 'letsencrypt_staging') return 'untrusted';
	return null;
}

export const PROBLEM_LABELS: Record<Exclude<CertificateProblem, null>, string> = {
	failing: 'Not issued',
	renewal_failing: 'Renewal failing',
	expiring: 'Expiring soon',
	expired: 'Expired',
	untrusted: 'Staging certificate'
};

/** Badge for the row: status, tone and label in one place. */
export function certificateBadge(
	cert: Certificate,
	now: number
): { status: string; tone: StatusTone; label: string } {
	const problem = certificateProblem(cert, now);
	switch (problem) {
		case 'expired':
			return { status: 'expired', tone: 'danger', label: PROBLEM_LABELS.expired };
		case 'failing':
			return {
				status: cert.status,
				tone: cert.status === 'failed' ? 'danger' : 'warning',
				label: cert.status === 'failed' ? 'Failed' : 'Waiting for DNS'
			};
		case 'expiring':
			return { status: 'expiring', tone: 'warning', label: PROBLEM_LABELS.expiring };
		case 'renewal_failing':
			return { status: 'degraded', tone: 'warning', label: PROBLEM_LABELS.renewal_failing };
		case 'untrusted':
			return { status: 'degraded', tone: 'warning', label: 'Untrusted (staging)' };
		default:
			if (cert.status === 'pending') {
				return { status: 'pending', tone: 'progress', label: 'Requesting' };
			}
			return { status: 'active', tone: 'success', label: 'Valid' };
	}
}

/** Who issued it, in words. */
export function issuerLabel(kind: IssuerKind, issuer: string | null): string {
	switch (kind) {
		case 'letsencrypt':
			return "Let's Encrypt";
		case 'letsencrypt_staging':
			return "Let's Encrypt staging";
		case 'custom':
			return issuer ?? 'Custom CA';
		case 'uploaded':
			return issuer ? `Uploaded · ${issuer}` : 'Uploaded';
		default:
			return '—';
	}
}

/** What the domain serves, as a label and (when there is a page for it) a link. */
export function targetLabel(target: CertificateTarget): { label: string; href: string | null } {
	switch (target.kind) {
		case 'dashboard':
			return { label: 'Tond dashboard', href: '/settings' };
		case 'app':
			return { label: target.app_name, href: `/apps/${target.app_id}` };
		case 'preview':
			return {
				label:
					target.pr_number !== null
						? `Preview of ${target.app_name} (PR #${target.pr_number})`
						: `Preview of ${target.app_name}`,
				href: `/apps/${target.app_id}?tab=settings&section=previews`
			};
		case 'wildcard': {
			const hosts = `${target.hosts} ${target.hosts === 1 ? 'host' : 'hosts'}`;
			if (target.purpose === 'apps') {
				return { label: `Wildcard for every app (${hosts})`, href: '/integrations/dns' };
			}
			if (target.purpose === 'previews') {
				return { label: `Wildcard for every preview (${hosts})`, href: '/integrations/dns' };
			}
			return { label: `Wildcard (${hosts})`, href: null };
		}
		default:
			return { label: 'Not in use', href: null };
	}
}

/** Uploaded by an admin: never renewed by Tond. */
export function isUploaded(cert: Certificate): boolean {
	return cert.cert_type === 'custom';
}

/** Show "Retry now" for rows that are waiting on a backoff or failing. */
export function canRetry(cert: Certificate, now: number): boolean {
	if (isUploaded(cert)) return false;
	const problem = certificateProblem(cert, now);
	return (
		problem === 'failing' ||
		problem === 'renewal_failing' ||
		problem === 'expiring' ||
		problem === 'expired'
	);
}

const PROBLEM_RANK: Record<Exclude<CertificateProblem, null>, number> = {
	expired: 0,
	failing: 1,
	expiring: 2,
	renewal_failing: 3,
	untrusted: 4
};

/** Problems first (worst first), then soonest expiry, then domain. */
export function sortCertificates(certs: Certificate[], now: number): Certificate[] {
	const rank = (c: Certificate) => {
		const p = certificateProblem(c, now);
		return p === null ? 10 : PROBLEM_RANK[p];
	};
	return [...certs].sort((a, b) => {
		const r = rank(a) - rank(b);
		if (r !== 0) return r;
		const da = hasIssuedCertificate(a) ? (daysLeft(a.expires_at, now) ?? 1e9) : 1e9;
		const db = hasIssuedCertificate(b) ? (daysLeft(b.expires_at, now) ?? 1e9) : 1e9;
		if (da !== db) return da - db;
		return a.domain.localeCompare(b.domain);
	});
}
