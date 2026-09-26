import { describe, expect, it } from 'vitest';
import type { ApiToken, User } from './api';
import {
	actionLabel,
	actorLabel,
	defaultExpiry,
	expiryOptions,
	hasScope,
	isAdminScope,
	isSecurityAction,
	parseIpList,
	passwordProblem,
	relativeTime,
	tokenState
} from './access';

const user = (extra: Partial<User>): User => ({
	id: 'u1',
	email: 'a@b.c',
	role: 'viewer',
	totp_enabled: false,
	created_at: '2026-01-01T00:00:00Z',
	...extra
});

const token = (extra: Partial<ApiToken>): ApiToken => ({
	id: 't1',
	name: 'ci',
	prefix: 'tond_pat_12345678',
	kind: 'ci',
	scopes: ['apps:read'],
	app_ids: [],
	project_ids: [],
	allowed_ips: [],
	owner_id: 'u1',
	owner_label: 'a@b.c',
	expires_at: null,
	last_used_at: null,
	last_used_ip: null,
	revoked_at: null,
	created_at: '2026-01-01T00:00:00Z',
	...extra
});

describe('hasScope', () => {
	it('uses the scopes the server sent', () => {
		const deployer = user({ role: 'deployer', scopes: ['apps:read', 'apps:deploy'] });
		expect(hasScope(deployer, 'apps:deploy')).toBe(true);
		expect(hasScope(deployer, 'settings:write')).toBe(false);
	});
	it('falls back to the role and refuses nobody signed in', () => {
		expect(hasScope(user({ role: 'admin' }), 'users:admin')).toBe(true);
		expect(hasScope(user({ role: 'viewer' }), 'users:admin')).toBe(false);
		expect(hasScope(null, 'apps:read')).toBe(false);
	});
});

describe('token lifetimes', () => {
	it('agents must expire within 30 days, others may never expire', () => {
		expect(expiryOptions('agent').every((o) => o.hours > 0 && o.hours <= 720)).toBe(true);
		expect(expiryOptions('ci').some((o) => o.hours === 0)).toBe(true);
		expect(defaultExpiry('agent')).toBe('24h');
		expect(defaultExpiry('personal')).toBe('90d');
	});
	it('knows which scopes agents can never get', () => {
		expect(isAdminScope('users:admin')).toBe(true);
		expect(isAdminScope('apps:deploy')).toBe(false);
	});
});

describe('tokenState', () => {
	const now = Date.parse('2026-06-01T00:00:00Z');
	it('reports revoked, expired, expiring and active tokens', () => {
		expect(tokenState(token({ revoked_at: '2026-05-01T00:00:00Z' }), now)).toBe('revoked');
		expect(tokenState(token({ expires_at: '2026-05-31T00:00:00Z' }), now)).toBe('expired');
		expect(tokenState(token({ expires_at: '2026-06-03T00:00:00Z' }), now)).toBe('expiring');
		expect(tokenState(token({ expires_at: '2026-09-01T00:00:00Z' }), now)).toBe('active');
		expect(tokenState(token({}), now)).toBe('active');
	});
});

describe('parseIpList', () => {
	it('accepts addresses and CIDR ranges, one per line or comma-separated', () => {
		expect(parseIpList('203.0.113.4\n10.0.0.0/8, 2001:db8::/32')).toEqual({
			ips: ['203.0.113.4', '10.0.0.0/8', '2001:db8::/32'],
			invalid: []
		});
	});
	it('flags bad entries', () => {
		expect(parseIpList('999.1.1.1 10.0.0.0/33 example.com').invalid).toEqual([
			'999.1.1.1',
			'10.0.0.0/33',
			'example.com'
		]);
		expect(parseIpList('  ').ips).toEqual([]);
	});
});

describe('relativeTime', () => {
	const now = Date.parse('2026-06-01T12:00:00Z');
	it('describes past and future times', () => {
		expect(relativeTime(null, now)).toBe('never');
		expect(relativeTime('2026-06-01T11:59:30Z', now)).toBe('just now');
		expect(relativeTime('2026-06-01T11:55:00Z', now)).toBe('5 min ago');
		expect(relativeTime('2026-06-01T10:00:00Z', now)).toBe('2 hours ago');
		expect(relativeTime('2026-06-04T12:00:00Z', now)).toBe('in 3 days');
	});
});

describe('audit labels', () => {
	it('turns actions and actors into plain language', () => {
		expect(actionLabel('token_create')).toBe('created an API token');
		expect(actionLabel('something_new')).toBe('something new');
		expect(actorLabel({ actor_kind: 'token', actor_label: 'ci' })).toBe('token ci');
		expect(actorLabel({ actor_kind: 'local_socket', actor_label: 'local-socket' })).toBe(
			'root (CLI on the server)'
		);
		expect(actorLabel({ actor_kind: 'user', actor_label: 'a@b.c' })).toBe('a@b.c');
	});
	it('recognises security events', () => {
		expect(isSecurityAction('login_failed')).toBe(true);
		expect(isSecurityAction('token_revoke')).toBe(true);
		expect(isSecurityAction('deploy')).toBe(false);
	});
});

describe('passwordProblem', () => {
	it('checks length only (the server checks breached passwords)', () => {
		expect(passwordProblem('short')).toMatch(/12 characters/);
		expect(passwordProblem('purple otter lamp')).toBeNull();
		expect(passwordProblem('x'.repeat(257))).toMatch(/256/);
	});
});
