import { describe, expect, it } from 'vitest';
import type { AppDomain, Certificate } from '$lib/api';
import {
	domainSummary,
	findCertificate,
	parseZones,
	providerFor,
	redirectTargets,
	validateDomainInput,
	wildcardFor,
	wwwCounterpart
} from './domains';

function row(domain: string, kind: AppDomain['kind'], extra: Partial<AppDomain> = {}): AppDomain {
	return {
		domain,
		kind,
		redirect_to: null,
		redirect_code: 308,
		service: null,
		port: null,
		created_at: '',
		automatic: false,
		warning: null,
		...extra
	};
}

const cert = (domain: string, status = 'active') => ({ domain, status }) as Certificate;

describe('domain helpers', () => {
	it('validates and normalizes what people type', () => {
		expect(validateDomainInput(' Shop.Example.com. ')).toEqual({ domain: 'shop.example.com' });
		expect(validateDomainInput('')).toEqual({ error: 'Enter a domain like app.example.com.' });
		expect(validateDomainInput('https://example.com')).toEqual({
			error: 'Enter just the domain, without https://.'
		});
		expect(validateDomainInput('localhost')).toHaveProperty('error');
	});

	it('finds the certificate that serves a host, own before wildcard', () => {
		const certs = [
			cert('*.apps.test'),
			cert('special.apps.test', 'pending'),
			cert('*.x.test', 'pending')
		];
		expect(findCertificate(certs, 'web.apps.test')).toEqual({
			cert: certs[0],
			coveredBy: '*.apps.test'
		});
		expect(findCertificate(certs, 'special.apps.test')).toEqual({
			cert: certs[1],
			coveredBy: null
		});
		expect(findCertificate(certs, 'a.b.apps.test').cert).toBeUndefined();
		expect(findCertificate(certs, 'a.x.test').cert).toBeUndefined();
		expect(wildcardFor('apps.test')).toBeNull();
	});

	it('pairs www and apex names', () => {
		expect(wwwCounterpart('example.com')).toBe('www.example.com');
		expect(wwwCounterpart('www.example.com')).toBe('example.com');
	});

	it('describes kinds and offers only serving domains as redirect targets', () => {
		const list = [
			row('example.com', 'primary'),
			row('web.apps.test', 'alias', { automatic: true }),
			row('www.example.com', 'redirect', { redirect_to: 'example.com', redirect_code: 301 })
		];
		expect(domainSummary(list[2])).toMatch(/example.com \(301\), keeping the path and query/);
		expect(domainSummary(list[1])).toMatch(/Automatic address/);
		expect(redirectTargets(list, 'web.apps.test')).toEqual(['example.com']);
	});

	it('parses zone lists and picks the most specific provider', () => {
		expect(parseZones('Example.com, *.apps.test\nexample.com')).toEqual([
			'example.com',
			'apps.test'
		]);
		const providers = [
			{ id: 'a', zones: ['example.com'] },
			{ id: 'b', zones: ['apps.example.com'] }
		];
		expect(providerFor(providers, '*.apps.example.com')?.provider.id).toBe('b');
		expect(providerFor(providers, 'x.example.com')?.zone).toBe('example.com');
		expect(providerFor(providers, 'badexample.com')).toBeNull();
	});
});
