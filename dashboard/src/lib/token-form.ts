// token-form.ts — state and validation for the "Create API token" form

import type { CreateTokenInput, Scope, TokenCatalog, TokenKind } from './api';
import { expiryOptions, isAdminScope, parseIpList } from './access';

export interface TokenFormState {
	name: string;
	kind: TokenKind;
	/** Preset id, or 'custom' once the scope list was edited by hand */
	preset: string;
	scopes: Scope[];
	/** 'all' or 'some' (limited to the chosen apps/projects) */
	reach: 'all' | 'some';
	appIds: string[];
	projectIds: string[];
	allowedIps: string;
	expiry: string;
}

/** Scopes a checkbox may be ticked for: within the caller's reach, never admin for agents. */
export function canTick(scope: Scope, kind: TokenKind, catalog: TokenCatalog): boolean {
	if (!catalog.grantable.includes(scope)) return false;
	return !(kind === 'agent' && isAdminScope(scope));
}

/** The first problem with the form, or null when it can be submitted. */
export function tokenFormProblem(form: TokenFormState, catalog: TokenCatalog): string | null {
	if (!form.name.trim()) return 'Give the token a name, e.g. the workflow or machine that uses it.';
	if (form.name.trim().length > 64) return 'Token names can be at most 64 characters.';
	if (form.scopes.length === 0) return 'Choose a preset or at least one scope.';
	const blocked = form.scopes.filter((s) => !canTick(s, form.kind, catalog));
	if (blocked.length) {
		return form.kind === 'agent' && blocked.some(isAdminScope)
			? `Agent tokens can't hold admin scopes (${blocked.join(', ')}).`
			: `You can't grant ${blocked.join(', ')}: a token never exceeds your own access.`;
	}
	if (form.reach === 'some' && form.appIds.length === 0 && form.projectIds.length === 0) {
		return 'Pick at least one app or project, or allow all apps.';
	}
	const { invalid } = parseIpList(form.allowedIps);
	if (invalid.length) return `Not an IP address or CIDR range: ${invalid.join(', ')}`;
	if (!expiryOptions(form.kind).some((o) => o.id === form.expiry)) {
		return 'Choose when the token expires.';
	}
	return null;
}

/** The API request for a valid form. */
export function tokenInput(form: TokenFormState): CreateTokenInput {
	const expiry = expiryOptions(form.kind).find((o) => o.id === form.expiry);
	return {
		name: form.name.trim(),
		kind: form.kind,
		scopes: form.scopes,
		app_ids: form.reach === 'some' ? form.appIds : [],
		project_ids: form.reach === 'some' ? form.projectIds : [],
		allowed_ips: parseIpList(form.allowedIps).ips,
		expires_in_hours: expiry?.hours ?? 24
	};
}

/** The preset whose scopes are exactly `scopes`, or 'custom'. */
export function matchingPreset(scopes: Scope[], catalog: TokenCatalog): string {
	const key = [...scopes].sort().join(' ');
	return catalog.presets.find((p) => [...p.scopes].sort().join(' ') === key)?.id ?? 'custom';
}
