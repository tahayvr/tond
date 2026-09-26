// Domain card and DNS provider logic: validation, labels, which certificate
// serves a host (its own or a wildcard), and the provider token instructions.

import type { AppDomain, Certificate, DnsProviderKind } from '$lib/api';

const HOSTNAME = /^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*[a-z0-9]$/;

/** Trim, lowercase and drop a trailing dot (as the server does). */
export function normalizeDomainInput(raw: string): string {
	return raw.trim().toLowerCase().replace(/\.$/, '');
}

/** A normalized domain, or the message to show under the input. */
export function validateDomainInput(raw: string): { domain: string } | { error: string } {
	const domain = normalizeDomainInput(raw);
	if (!domain) return { error: 'Enter a domain like app.example.com.' };
	if (/^https?:\/\//.test(domain)) {
		return { error: 'Enter just the domain, without https://.' };
	}
	if (!HOSTNAME.test(domain)) return { error: `"${domain}" isn't a valid domain name.` };
	return { domain };
}

/** `a.b.c` → `*.b.c` (null when the parent is a bare TLD). */
export function wildcardFor(host: string): string | null {
	const dot = host.indexOf('.');
	if (dot === -1) return null;
	const parent = host.slice(dot + 1);
	return parent.includes('.') ? `*.${parent}` : null;
}

/** The certificate that serves `domain`: its own row, else an active wildcard one label up. */
export function findCertificate(
	certs: Certificate[],
	domain: string
): { cert: Certificate | undefined; coveredBy: string | null } {
	const own = certs.find((c) => c.domain === domain);
	if (own) return { cert: own, coveredBy: null };
	const wildcard = wildcardFor(domain);
	const covering = wildcard
		? certs.find((c) => c.domain === wildcard && c.status === 'active')
		: undefined;
	return { cert: covering, coveredBy: covering ? covering.domain : null };
}

/** The other name of a www/apex pair. */
export function wwwCounterpart(domain: string): string {
	return domain.startsWith('www.') ? domain.slice(4) : `www.${domain}`;
}

export const KIND_LABELS: Record<AppDomain['kind'], string> = {
	primary: 'Primary',
	alias: 'Alias',
	redirect: 'Redirect'
};

/** One line under a domain in the list. */
export function domainSummary(d: AppDomain): string {
	switch (d.kind) {
		case 'primary':
			return d.automatic
				? 'Automatic address under the apps domain. The main address of this app.'
				: 'The main address of this app.';
		case 'alias':
			return d.automatic
				? 'Automatic address under the apps domain. Serves the app too.'
				: 'Serves the app too.';
		default:
			return `Sends visitors to ${d.redirect_to} (${d.redirect_code === 301 ? '301' : '308'}), keeping the path and query.`;
	}
}

/** Domains a redirect may point at: the app's serving domains, except `self`. */
export function redirectTargets(domains: AppDomain[], self?: string): string[] {
	return domains.filter((d) => d.kind !== 'redirect' && d.domain !== self).map((d) => d.domain);
}

/** Primary first, then aliases, then redirects (the server's order). */
export function sortDomains(domains: AppDomain[]): AppDomain[] {
	const rank = { primary: 0, alias: 1, redirect: 2 } as const;
	return [...domains].sort((a, b) => rank[a.kind] - rank[b.kind]);
}

// ── DNS providers ────────────────────────────────────────────────────────────

export interface ProviderHelp {
	label: string;
	/** Label of the secret field */
	tokenLabel: string;
	/** Scoped-token instructions, one step per line */
	steps: string[];
	docs: string;
	/** Route 53 also needs an access key id */
	needsKeyId: boolean;
}

export const PROVIDER_HELP: Record<DnsProviderKind, ProviderHelp> = {
	cloudflare: {
		label: 'Cloudflare',
		tokenLabel: 'API token',
		steps: [
			'Cloudflare dashboard → My Profile → API Tokens → Create Token.',
			'Start from the "Edit zone DNS" template.',
			'Permissions: Zone → DNS → Edit. Zone Resources: Include → Specific zone → your zone (not "All zones").',
			'Create the token and paste it here. Tond only adds and removes _acme-challenge TXT records.'
		],
		docs: 'https://developers.cloudflare.com/fundamentals/api/get-started/create-token/',
		needsKeyId: false
	},
	hetzner: {
		label: 'Hetzner DNS',
		tokenLabel: 'API token',
		steps: [
			'Hetzner Console → the project that holds your DNS zone → Security → API Tokens.',
			'Generate a token with Read & Write permission.',
			'A project token can change every zone in that project: keep DNS zones in a project of their own if you can.'
		],
		docs: 'https://docs.hetzner.cloud/reference/cloud#zones',
		needsKeyId: false
	},
	digitalocean: {
		label: 'DigitalOcean',
		tokenLabel: 'Personal access token',
		steps: [
			'DigitalOcean control panel → API → Tokens → Generate New Token.',
			'Choose "Custom scopes" and grant only domain: create, read, delete.',
			'Set an expiry you are comfortable rotating, then paste the token here.'
		],
		docs: 'https://docs.digitalocean.com/reference/api/create-personal-access-token/',
		needsKeyId: false
	},
	route53: {
		label: 'Amazon Route 53',
		tokenLabel: 'Secret access key',
		steps: [
			'IAM → Users → create a user for Tond with programmatic access only.',
			'Attach a policy allowing route53:ListHostedZonesByName on *, and route53:ChangeResourceRecordSets on arn:aws:route53:::hostedzone/<your zone id>.',
			'Create an access key for the user: the access key id and the secret access key go below.'
		],
		docs: 'https://docs.aws.amazon.com/Route53/latest/DeveloperGuide/access-control-managing-permissions.html',
		needsKeyId: true
	},
	challtestsrv: {
		label: 'challtestsrv (test)',
		tokenLabel: 'Token (unused)',
		steps: [
			"Pebble's test DNS server. Only available when tond-server runs with TOND_DNS_TEST_PROVIDER=1."
		],
		docs: 'https://github.com/letsencrypt/pebble',
		needsKeyId: false
	}
};

/** The provider (and zone) that covers `domain`: the longest matching zone. */
export function providerFor<P extends { zones: string[] }>(
	providers: P[],
	domain: string
): { provider: P; zone: string } | null {
	const host = domain.replace(/^\*\./, '');
	let best: { provider: P; zone: string } | null = null;
	for (const provider of providers) {
		for (const zone of provider.zones) {
			if ((host === zone || host.endsWith(`.${zone}`)) && zone.length > (best?.zone.length ?? 0)) {
				best = { provider, zone };
			}
		}
	}
	return best;
}

/** Split "example.com, apps.example.com" or one per line into zone names. */
export function parseZones(raw: string): string[] {
	return [
		...new Set(
			raw
				.split(/[\s,]+/)
				.map((z) => normalizeDomainInput(z.replace(/^\*\./, '')))
				.filter(Boolean)
		)
	];
}
