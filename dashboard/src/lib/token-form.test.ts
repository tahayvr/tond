import { describe, expect, it } from 'vitest';
import type { TokenCatalog } from './api';
import {
	canTick,
	matchingPreset,
	tokenFormProblem,
	tokenInput,
	type TokenFormState
} from './token-form';

const catalog: TokenCatalog = {
	groups: [],
	presets: [
		{ id: 'readonly', label: 'Read-only', description: '', scopes: ['apps:read', 'logs:read'] },
		{ id: 'deployer', label: 'Deployer', description: '', scopes: ['apps:read', 'apps:deploy'] }
	],
	roles: [],
	grantable: ['apps:read', 'apps:deploy', 'logs:read', 'users:admin']
};

const form = (extra: Partial<TokenFormState> = {}): TokenFormState => ({
	name: 'ci',
	kind: 'ci',
	preset: 'deployer',
	scopes: ['apps:read', 'apps:deploy'],
	reach: 'all',
	appIds: [],
	projectIds: [],
	allowedIps: '',
	expiry: '90d',
	...extra
});

describe('token form', () => {
	it('accepts a complete form and builds the request', () => {
		expect(tokenFormProblem(form(), catalog)).toBeNull();
		expect(tokenInput(form({ allowedIps: '10.0.0.0/8\n203.0.113.4' }))).toEqual({
			name: 'ci',
			kind: 'ci',
			scopes: ['apps:read', 'apps:deploy'],
			app_ids: [],
			project_ids: [],
			allowed_ips: ['10.0.0.0/8', '203.0.113.4'],
			expires_in_hours: 2160
		});
	});

	it('only sends the resource filter when limited', () => {
		const limited = form({ reach: 'some', appIds: ['app-1'] });
		expect(tokenInput(limited).app_ids).toEqual(['app-1']);
		expect(tokenInput({ ...limited, reach: 'all' }).app_ids).toEqual([]);
		expect(tokenFormProblem(form({ reach: 'some' }), catalog)).toMatch(/at least one app/);
	});

	it('explains every problem in plain words', () => {
		expect(tokenFormProblem(form({ name: ' ' }), catalog)).toMatch(/name/);
		expect(tokenFormProblem(form({ scopes: [] }), catalog)).toMatch(/at least one scope/);
		expect(tokenFormProblem(form({ scopes: ['db:write'] }), catalog)).toMatch(
			/exceeds your own access/
		);
		expect(tokenFormProblem(form({ allowedIps: 'nope' }), catalog)).toMatch(/nope/);
		expect(
			tokenFormProblem(form({ kind: 'agent', expiry: '24h', scopes: ['users:admin'] }), catalog)
		).toMatch(/Agent tokens/);
		expect(tokenFormProblem(form({ kind: 'agent', expiry: 'never' }), catalog)).toMatch(/expires/);
	});

	it('never offers admin scopes to agents or scopes beyond the caller', () => {
		expect(canTick('users:admin', 'ci', catalog)).toBe(true);
		expect(canTick('users:admin', 'agent', catalog)).toBe(false);
		expect(canTick('db:write', 'ci', catalog)).toBe(false);
	});

	it('recognises presets regardless of order', () => {
		expect(matchingPreset(['apps:deploy', 'apps:read'], catalog)).toBe('deployer');
		expect(matchingPreset(['apps:read'], catalog)).toBe('custom');
	});
});
