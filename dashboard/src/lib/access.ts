// access.ts — roles, scopes, API token and audit helpers for the dashboard
//
// The server is the authority on permissions (it refuses anything outside the
// caller's scopes with 403 SCOPE_DENIED); these helpers only decide what the
// UI offers. The scope catalog (descriptions, presets) comes from
// GET /api/tokens/catalog; ALL_SCOPES mirrors it for the dev auth bypass.

import type { ApiToken, AuditLogEntry, Role, Scope, TokenKind, User } from './api';

export const ALL_SCOPES: Scope[] = [
	'apps:read',
	'apps:deploy',
	'apps:write',
	'apps:control',
	'apps:rollback',
	'apps:delete',
	'env:read-names',
	'env:read-values',
	'env:write',
	'db:credentials',
	'db:read',
	'db:write',
	'db:delete',
	'snapshots:read',
	'snapshots:create',
	'snapshots:restore',
	'logs:read',
	'metrics:read',
	'events:read',
	'cron:read',
	'cron:write',
	'cron:trigger',
	'previews:write',
	'settings:write',
	'users:admin',
	'tokens:admin',
	'terminal:host'
];

export const ROLE_LABELS: Record<Role, string> = {
	admin: 'Admin',
	deployer: 'Deployer',
	viewer: 'Viewer'
};

export const ROLE_SUMMARIES: Record<Role, string> = {
	admin: 'Everything, including users, API tokens and server settings.',
	deployer: 'Deploy, restart and roll back apps; previews; env vars without reading values.',
	viewer: 'Read-only: apps, logs, metrics and activity. No secret values.'
};

/** Whether the user holds `scope` (falls back to the role for older payloads). */
export function hasScope(user: User | null | undefined, scope: Scope): boolean {
	if (!user) return false;
	if (user.scopes) return user.scopes.includes(scope);
	return user.role === 'admin';
}

// ── Token kinds and lifetimes ──

export const TOKEN_KINDS: { id: TokenKind; label: string; hint: string }[] = [
	{ id: 'personal', label: 'Personal', hint: 'Your own scripts and the tond CLI' },
	{ id: 'ci', label: 'CI', hint: 'A build pipeline that deploys' },
	{ id: 'service', label: 'Service', hint: 'Another server or integration' },
	{ id: 'agent', label: 'Agent', hint: 'An AI agent or bot: must expire, no admin scopes' }
];

export interface ExpiryOption {
	id: string;
	label: string;
	/** Lifetime in hours; 0 = never expires */
	hours: number;
}

const AGENT_EXPIRY: ExpiryOption[] = [
	{ id: '1h', label: '1 hour', hours: 1 },
	{ id: '24h', label: '24 hours', hours: 24 },
	{ id: '7d', label: '7 days', hours: 24 * 7 },
	{ id: '30d', label: '30 days (maximum)', hours: 24 * 30 }
];

const OTHER_EXPIRY: ExpiryOption[] = [
	{ id: '7d', label: '7 days', hours: 24 * 7 },
	{ id: '30d', label: '30 days', hours: 24 * 30 },
	{ id: '90d', label: '90 days', hours: 24 * 90 },
	{ id: '1y', label: '1 year', hours: 24 * 365 },
	{ id: 'never', label: 'Never expires', hours: 0 }
];

/** Lifetimes offered for a token kind (agents must expire within 30 days). */
export function expiryOptions(kind: TokenKind): ExpiryOption[] {
	return kind === 'agent' ? AGENT_EXPIRY : OTHER_EXPIRY;
}

/** Default lifetime for a kind: 24 h for agents, 90 days otherwise. */
export function defaultExpiry(kind: TokenKind): string {
	return kind === 'agent' ? '24h' : '90d';
}

/** Scopes an agent token may never hold. */
export function isAdminScope(scope: Scope): boolean {
	return ['settings:write', 'users:admin', 'tokens:admin', 'terminal:host'].includes(scope);
}

export type TokenState = 'active' | 'expiring' | 'expired' | 'revoked';

/** Where a token stands: revoked, expired, expiring within 7 days, or active. */
export function tokenState(token: ApiToken, now: number = Date.now()): TokenState {
	if (token.revoked_at) return 'revoked';
	if (token.expires_at) {
		const expires = Date.parse(token.expires_at);
		if (expires <= now) return 'expired';
		if (expires - now < 7 * 24 * 3600 * 1000) return 'expiring';
	}
	return 'active';
}

/** Parse the allowed-IPs textarea (one per line or comma-separated). */
export function parseIpList(text: string): { ips: string[]; invalid: string[] } {
	const entries = text
		.split(/[\s,]+/)
		.map((s) => s.trim())
		.filter(Boolean);
	const invalid = entries.filter((e) => !isIpOrCidr(e));
	return { ips: entries, invalid };
}

function isIpOrCidr(value: string): boolean {
	const [addr, prefix, ...rest] = value.split('/');
	if (rest.length) return false;
	const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(addr);
	if (v4) {
		if (v4.slice(1).some((o) => Number(o) > 255)) return false;
		return prefix === undefined || (/^\d+$/.test(prefix) && Number(prefix) <= 32);
	}
	if (/^[0-9a-f:]+$/i.test(addr) && addr.includes(':')) {
		return prefix === undefined || (/^\d+$/.test(prefix) && Number(prefix) <= 128);
	}
	return false;
}

// ── Time ──

/** "just now", "5 min ago", "in 3 days", … */
export function relativeTime(iso: string | null | undefined, now: number = Date.now()): string {
	if (!iso) return 'never';
	const t = Date.parse(iso);
	if (Number.isNaN(t)) return iso;
	const diff = t - now;
	const abs = Math.abs(diff);
	const units: [number, string][] = [
		[365 * 24 * 3600 * 1000, 'year'],
		[30 * 24 * 3600 * 1000, 'month'],
		[24 * 3600 * 1000, 'day'],
		[3600 * 1000, 'hour'],
		[60 * 1000, 'min']
	];
	if (abs < 60 * 1000) return diff < 0 ? 'just now' : 'in under a minute';
	for (const [ms, name] of units) {
		if (abs >= ms) {
			const n = Math.floor(abs / ms);
			const label = name === 'min' ? 'min' : `${name}${n === 1 ? '' : 's'}`;
			return diff < 0 ? `${n} ${label} ago` : `in ${n} ${label}`;
		}
	}
	return iso;
}

// ── Audit log ──

const ACTION_LABELS: Record<string, string> = {
	login: 'signed in',
	login_failed: 'failed to sign in',
	logout: 'signed out',
	admin_setup: 'created the admin account',
	password_change: 'changed their password',
	password_reset: 'reset their password',
	password_reset_link: 'issued a password reset link',
	invite_accepted: 'accepted an invite',
	'2fa_enabled': 'turned on two-factor auth',
	'2fa_disabled': 'turned off two-factor auth',
	session_revoke: 'revoked sessions',
	session_revoke_others: 'signed out other sessions',
	user_invite: 'invited a user',
	user_invite_resend: 'issued a new invite link',
	user_role_change: 'changed a role',
	user_disable: 'disabled a user',
	user_enable: 'enabled a user',
	user_delete: 'deleted a user',
	token_create: 'created an API token',
	token_revoke: 'revoked an API token',
	settings_update: 'changed server settings',
	host_terminal_enabled: 'turned on the host terminal',
	host_terminal_disabled: 'turned off the host terminal',
	terminal_opened: 'opened a terminal',
	terminal_closed: 'closed a terminal',
	env_reveal: 'revealed an env var',
	env_update: 'changed env vars',
	create: 'created',
	delete: 'deleted',
	deploy: 'deployed',
	rollback: 'rolled back',
	stop: 'stopped',
	start: 'started',
	restart: 'restarted',
	snapshot_restore_full: 'restored the whole server',
	snapshot_restore_app: 'restored an app from a snapshot',
	snapshot_restore_database: 'restored a database from a snapshot',
	audit_pruned: 'pruned old activity (retention)'
};

/** A plain-language verb phrase for an action ("deployed", "revoked an API token"). */
export function actionLabel(action: string): string {
	return ACTION_LABELS[action] ?? action.replaceAll('_', ' ');
}

/** How an audit actor is shown: an email, "token ci", "root (CLI)", "system". */
export function actorLabel(entry: Pick<AuditLogEntry, 'actor_kind' | 'actor_label'>): string {
	switch (entry.actor_kind) {
		case 'token':
			return `token ${entry.actor_label}`;
		case 'local_socket':
			return 'root (CLI on the server)';
		case 'system':
			return `system (${entry.actor_label})`;
		default:
			return entry.actor_label;
	}
}

/** Whether an action is a security event (sign-ins, users, tokens, 2FA, …). */
export function isSecurityAction(action: string): boolean {
	return /^(login|logout|admin_setup|password|invite|2fa|session|user_|token_|host_terminal|terminal|env_reveal|settings|audit_)/.test(
		action
	);
}

// ── Passwords ──

/** Client-side hint for the one password policy (the server also refuses breached passwords). */
export function passwordProblem(password: string): string | null {
	const length = [...password].length;
	if (length < 12) return 'Use at least 12 characters. A few random words work well.';
	if (length > 256) return 'Use at most 256 characters.';
	return null;
}
