import { connectWithRetry, wsUrl, type ReconnectingSocket } from './reconnecting-socket';

export interface DeployLog {
	level: string;
	message: string;
	trigger_source: string;
	created_at: string;
}

/**
 * A persistent volume. The server chooses the host side:
 * `<data>/volumes/<app>/<name>` — `host_path` is informational.
 */
export interface Volume {
	name: string;
	host_path: string;
	container_path: string;
}

/** A volume as sent to the API: mount point plus optional directory name. */
export interface VolumeInput {
	container_path: string;
	/** Directory under the app's volume folder; derived from container_path when omitted */
	name?: string;
}

/** A cron volume: the job's own directory, or a volume shared from an app. */
export interface CronVolume {
	/** Share this app's volume instead of the job's own directory */
	app?: string | null;
	name: string;
	/** Computed by the server (read-only) */
	host_path: string;
	container_path: string;
}

export interface CronVolumeInput {
	app?: string | null;
	name?: string;
	container_path: string;
}

export interface App {
	id: string;
	name: string;
	domain: string | null;
	/**
	 * Set on GET /apps/{id} when the domain is on the same site as the dashboard
	 * (allowed, but shown as a warning).
	 */
	domain_warning?: string | null;
	source_type: string;
	source_url: string | null;
	status: string;
	container_port: number;
	host_port: number | null;
	memory_limit: number | null;
	cpu_limit: number | null;
	health_check_path: string | null;
	health_check_interval: number;
	health_failure_threshold: number;
	health_consecutive_failures: number;
	volumes: Volume[];
	drain_timeout: number;
	/** "tbuild" | "dockerfile" for source-building apps */
	build_method: string | null;
	// Git integration (Phase 4)
	auto_deploy: boolean;
	watch_branch: string | null;
	deploy_key_pub: string | null;
	/** Registry credential attached for private image pulls */
	registry_credential_id: string | null;
	/** GitHub App integration (Phase 4.4) */
	github_app_id: string | null;
	// Project grouping (Phase 6.1)
	project_id: string | null;
	/** Raw Dockerfile content (source_type='dockerfile') */
	dockerfile_content: string | null;
	/** Raw docker-compose.yml content (source_type='compose') */
	compose_content: string | null;
	/** Subdirectory to use as build root (monorepo support) */
	base_dir: string | null;
	/** Whether build output is served as a static site */
	static_build: boolean;
	/** Phase 4.6: opt-in to automatic PR preview deployments via GitHub App */
	preview_enabled: boolean;
	/** App env var keys shared with preview containers (all others are withheld) */
	preview_shared_keys: string[];
	/**
	 * How a deploy decides the new container is ready: `http` (health path, or
	 * any HTTP answer on the port), `tcp` (the port accepts connections) or
	 * `none` (background worker: no port, no domain, never routed).
	 */
	readiness?: Readiness;
	/** For `none`: seconds the container must stay up */
	ready_after_secs?: number;
	created_at: string;
	updated_at: string;
}

export type Readiness = 'http' | 'tcp' | 'none';

/** An app's readiness mode (`http` for apps from before the setting). */
export function appReadiness(app: Pick<App, 'readiness'>): Readiness {
	return app.readiness === 'tcp' || app.readiness === 'none' ? app.readiness : 'http';
}

export interface TemplateProject {
	name: string;
	description: string | null;
}

export interface TemplateSource {
	kind: string;
	url: string;
	ref: string | null;
}

export interface TemplateVolume {
	container_path: string;
	description: string;
}

export interface TemplateDeploy {
	method: string;
	default_port: number;
	health_check_path: string | null;
	volumes: TemplateVolume[];
}

export interface TemplateEnvVar {
	key: string;
	description: string;
	required: boolean | null;
	value: string | null;
}

export interface TemplateDependency {
	name: string | null;
	kind: string;
	engine: string | null;
	required: boolean | null;
}

export interface TemplateCatalogEntry {
	id: string;
	name: string;
	description: string;
	category: string;
	tags: string[];
	maintainers: string[];
	project: TemplateProject | null;
	source: TemplateSource;
	deploy: TemplateDeploy;
	required_env: TemplateEnvVar[];
	optional_env: TemplateEnvVar[];
	dependencies: TemplateDependency[];
	recommended_domains: string[];
	post_deploy_checks: string[];
	setup_notes: string | null;
}

/** A user's role. Admin is the owner; see `Role::scopes` on the server. */
export type Role = 'admin' | 'deployer' | 'viewer';

/** A permission, e.g. `apps:deploy` (full list: GET /api/tokens/catalog). */
export type Scope = string;

export interface User {
	id: string;
	email: string;
	role: Role;
	/** Whether TOTP two-factor auth is active for this user */
	totp_enabled: boolean;
	created_at: string;
	/** Whether an admin has disabled the account */
	disabled?: boolean;
	/** False while an invited user hasn't set a password yet */
	password_set?: boolean;
	last_login_at?: string | null;
	/** The scopes the user's role grants (drives what the UI offers) */
	scopes?: Scope[];
}

export interface AuthStatus {
	authenticated: boolean;
	user: User | null;
}

interface ApiResponse<T> {
	success: boolean;
	data: T | null;
	error: string | null;
	/** Stable error code on some failures, e.g. SCOPE_DENIED, TOKEN_EXPIRED */
	code?: string;
}

const BASE = '/api';

/**
 * Sent on every API request. The server requires it on cookie-authenticated
 * mutations: browsers only let a page add a custom header to a cross-origin
 * request after a CORS preflight, which tond-server never grants, so its
 * presence proves the request came from the dashboard itself.
 */
export const CSRF_HEADER = 'X-Tond-CSRF';

export const SERVER_UNAVAILABLE_MESSAGE =
	'Server unavailable. Tond may be restarting — try again in a moment.';
export const SESSION_EXPIRED_MESSAGE = 'Your session has expired. Sign in again to continue.';

/** An API failure, carrying the HTTP status (0 = the request never got a response). */
export class ApiError extends Error {
	readonly status: number;
	/** Seconds to wait before retrying, from a 429's Retry-After header. */
	readonly retryAfter: number | null;
	/** Stable error code from the server (e.g. `SCOPE_DENIED`), when it sent one. */
	readonly code: string | null;

	constructor(
		message: string,
		status: number,
		retryAfter: number | null = null,
		code: string | null = null
	) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.retryAfter = retryAfter;
		this.code = code;
	}
}

// Endpoints that answer 401 for a wrong password or code. A 401 from these is
// an input error, not an expired session, so it must not sign the user out.
// (/terminal/tickets answers 401 for a wrong step-up password; a truly expired
// session is still caught by the next ordinary request.)
const CREDENTIAL_ENDPOINTS = [
	'/auth/status',
	'/auth/login',
	'/auth/logout',
	'/auth/setup',
	'/auth/setup/check',
	'/auth/2fa/setup',
	'/auth/2fa/verify',
	'/auth/2fa/confirm',
	'/auth/2fa/disable',
	'/auth/change-password',
	'/auth/password-link/check',
	'/auth/password-link/complete',
	'/terminal/tickets'
];

let unauthorizedHandler: (() => void) | null = null;

/** Register what happens when the session expires (auth.svelte.ts signs the user out). */
export function setUnauthorizedHandler(handler: (() => void) | null) {
	unauthorizedHandler = handler;
}

/** Parse a Retry-After header (delta-seconds or HTTP date) into whole seconds. */
export function parseRetryAfter(header: string | null, now: number = Date.now()): number | null {
	if (!header) return null;
	const trimmed = header.trim();
	if (/^\d+$/.test(trimmed)) return Number(trimmed);
	const date = Date.parse(trimmed);
	if (Number.isNaN(date)) return null;
	return Math.max(0, Math.ceil((date - now) / 1000));
}

/** Turn a fetch Response into data or a descriptive ApiError. */
async function parseResponse<T>(path: string, res: Response): Promise<T> {
	const endpoint = path.split('?')[0];

	if (res.status === 401 && !CREDENTIAL_ENDPOINTS.includes(endpoint)) {
		unauthorizedHandler?.();
		throw new ApiError(SESSION_EXPIRED_MESSAGE, 401);
	}

	if (res.status === 429) {
		const retryAfter = parseRetryAfter(res.headers.get('Retry-After'));
		const message =
			retryAfter !== null
				? `Too many attempts, try again in ${retryAfter} s.`
				: 'Too many attempts, try again in a moment.';
		throw new ApiError(message, 429, retryAfter);
	}

	// A proxy error page (502/504 HTML) or an empty body is not our API talking.
	// A 5xx without our JSON body means the server (or proxy) is down; any
	// other non-JSON reply is reported with its status.
	const notApi = () =>
		new ApiError(
			res.status === 0 || res.status >= 500
				? SERVER_UNAVAILABLE_MESSAGE
				: `Request failed (HTTP ${res.status})`,
			res.status
		);
	let json: ApiResponse<T>;
	try {
		json = await res.json();
	} catch {
		throw notApi();
	}
	if (!json || typeof json !== 'object' || !('success' in json)) {
		throw notApi();
	}
	if (!res.ok || !json.success) {
		throw new ApiError(
			json.error || `Request failed (HTTP ${res.status})`,
			res.status,
			null,
			json.code ?? null
		);
	}
	return json.data as T;
}

async function request<T>(path: string, options?: RequestInit): Promise<T> {
	const headers = new Headers(options?.headers);
	if (!headers.has('Content-Type')) headers.set('Content-Type', 'application/json');
	headers.set(CSRF_HEADER, '1');
	let res: Response;
	try {
		res = await fetch(`${BASE}${path}`, {
			credentials: 'same-origin',
			...options,
			headers
		});
	} catch {
		// Network failure: server down, connection reset, offline.
		throw new ApiError(SERVER_UNAVAILABLE_MESSAGE, 0);
	}
	return parseResponse<T>(path, res);
}

export async function listApps(): Promise<App[]> {
	return request<App[]>('/apps');
}

export async function listTemplates(): Promise<TemplateCatalogEntry[]> {
	return request<TemplateCatalogEntry[]>('/templates');
}

export async function getApp(id: string): Promise<App> {
	return request<App>(`/apps/${id}`);
}

export async function createApp(data: {
	name: string;
	/** "git" | "image" | "dockerfile" | "compose" */
	source_type: string;
	source_url?: string;
	branch?: string;
	container_port?: number;
	domain?: string;
	project_id?: string;
	memory_limit?: number;
	cpu_limit?: number;
	build_method?: string;
	auto_deploy?: boolean;
	watch_branch?: string;
	health_check_path?: string;
	health_check_interval?: number;
	health_failure_threshold?: number;
	drain_timeout?: number;
	env_vars?: Record<string, string>;
	volumes?: VolumeInput[];
	registry_credential_id?: string;
	github_app_id?: string;
	/** Raw Dockerfile content (required when source_type='dockerfile') */
	dockerfile_content?: string;
	/** Raw docker-compose.yml content (required when source_type='compose') */
	compose_content?: string;
	/** Subdirectory to use as build root (monorepo support) */
	base_dir?: string;
	/** Serve build output as a static site */
	static_build?: boolean;
	/** `http` (default) | `tcp` | `none` (background worker: no port, no domain) */
	readiness?: Readiness;
	/** For `none`: seconds the container must stay up (default 10) */
	ready_after_secs?: number;
}): Promise<App> {
	return request<App>('/apps', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function deleteApp(id: string): Promise<void> {
	await request(`/apps/${id}`, { method: 'DELETE' });
}

/** Start a deployment; the result carries its id for the live deploy view. */
export async function deployApp(id: string): Promise<DeployStarted> {
	return request<DeployStarted>(`/apps/${id}/deploy`, { method: 'POST' });
}

export async function stopApp(id: string): Promise<void> {
	await request(`/apps/${id}/stop`, { method: 'POST' });
}

export async function startApp(id: string): Promise<void> {
	await request(`/apps/${id}/start`, { method: 'POST' });
}

export async function restartApp(id: string): Promise<void> {
	await request(`/apps/${id}/restart`, { method: 'POST' });
}

export async function getAppLogs(
	id: string
): Promise<{ deploy_logs: DeployLog[]; container_logs: { ts: string | null; line: string }[] }> {
	return request(`/apps/${id}/logs`);
}

// ── App environment variables ──
// Values are secrets: the list endpoint returns masked values only, a single
// value is fetched with an explicit reveal, and writes are per-key patches.

export interface EnvVarEntry {
	key: string;
	/** Masked placeholder for a non-empty value, '' for an empty value */
	masked: string;
}

export interface AppEnv {
	vars: EnvVarEntry[];
	preview: {
		enabled: boolean;
		/**
		 * App variables preview containers receive. Every other variable is withheld:
		 * previews run pull-request code.
		 */
		shared_keys: string[];
		/** Values that replace or add variables in preview containers only */
		overrides: EnvVarEntry[];
	};
}

export interface EnvPatch {
	set?: Record<string, string>;
	unset?: string[];
}

export interface PreviewEnvPatch extends EnvPatch {
	/** App variables to share with previews */
	share?: string[];
	/** App variables to stop sharing with previews */
	unshare?: string[];
}

export async function getAppEnv(id: string): Promise<AppEnv> {
	return request<AppEnv>(`/apps/${id}/env`);
}

/** Set and remove individual variables; variables not named are left unchanged. */
export async function patchAppEnv(id: string, patch: EnvPatch): Promise<AppEnv> {
	return request<AppEnv>(`/apps/${id}/env`, {
		method: 'PATCH',
		body: JSON.stringify(patch)
	});
}

export async function revealAppEnvVar(id: string, key: string): Promise<string> {
	const res = await request<{ key: string; value: string }>(
		`/apps/${id}/env/${encodeURIComponent(key)}`
	);
	return res.value;
}

export async function patchPreviewEnv(id: string, patch: PreviewEnvPatch): Promise<AppEnv> {
	return request<AppEnv>(`/apps/${id}/preview-env`, {
		method: 'PATCH',
		body: JSON.stringify(patch)
	});
}

export async function revealPreviewEnvVar(id: string, key: string): Promise<string> {
	const res = await request<{ key: string; value: string }>(
		`/apps/${id}/preview-env/${encodeURIComponent(key)}`
	);
	return res.value;
}

export interface UpdateDomainResult {
	/** The domain as saved (trimmed, lowercased) */
	domain: string;
	/** Set when the domain is on the same site as the dashboard */
	warning: string | null;
}

export async function updateDomain(id: string, domain: string): Promise<UpdateDomainResult> {
	return request<UpdateDomainResult>(`/apps/${id}/domain`, {
		method: 'PUT',
		body: JSON.stringify({ domain })
	});
}

// ── Certificates ──

export interface Certificate {
	id: string;
	domain: string;
	cert_type: string;
	/** "active" | "pending" | "expired" | "failed" */
	status: string;
	issued_at: string;
	expires_at: string;
	/** Why the last issuance attempt failed or was skipped (null after success) */
	last_error: string | null;
	last_attempt_at: string | null;
	/** No new attempt before this time (exponential backoff) */
	next_retry_at: string | null;
	failure_count: number;
}

export async function listCertificates(): Promise<Certificate[]> {
	return request<Certificate[]>('/certs');
}

export async function retryCertificate(domain: string): Promise<void> {
	await request(`/certs/${encodeURIComponent(domain)}/retry`, { method: 'POST' });
}

export async function updateHealth(
	id: string,
	health_check_path: string | null,
	health_check_interval: number,
	health_failure_threshold: number
): Promise<void> {
	await request(`/apps/${id}/health`, {
		method: 'PUT',
		body: JSON.stringify({ health_check_path, health_check_interval, health_failure_threshold })
	});
}

/**
 * Set how deploys decide the app is ready. Switching to `none` (background
 * worker) removes the app's domain and health check path.
 */
export async function updateReadiness(
	id: string,
	readiness: Readiness,
	ready_after_secs?: number
): Promise<App> {
	return request<App>(`/apps/${id}/readiness`, {
		method: 'PUT',
		body: JSON.stringify({ readiness, ...(ready_after_secs ? { ready_after_secs } : {}) })
	});
}

/** Set the port the app listens on inside its container (applies on the next deploy). */
export async function updateContainerPort(id: string, container_port: number): Promise<App> {
	return request<App>(`/apps/${id}/port`, {
		method: 'PUT',
		body: JSON.stringify({ container_port })
	});
}

export async function updateDrainTimeout(id: string, drain_timeout: number): Promise<void> {
	await request(`/apps/${id}/drain-timeout`, {
		method: 'PUT',
		body: JSON.stringify({ drain_timeout })
	});
}

export async function updateAppResources(
	id: string,
	memory_limit: number | null,
	cpu_limit: number | null
): Promise<App> {
	return request<App>(`/apps/${id}/resources`, {
		method: 'PATCH',
		body: JSON.stringify({ memory_limit, cpu_limit })
	});
}

export interface ComposeValidation {
	valid: boolean;
	/** One human-readable line per problem (privileged, host binds, …) */
	errors: string[];
}

/** Check a compose file against Tond's policy without creating anything. */
export async function validateCompose(
	composeContent: string,
	appName?: string
): Promise<ComposeValidation> {
	return request<ComposeValidation>('/compose/validate', {
		method: 'POST',
		body: JSON.stringify({ compose_content: composeContent, app_name: appName || null })
	});
}

export async function addVolume(id: string, volume: VolumeInput): Promise<void> {
	await request(`/apps/${id}/volumes`, {
		method: 'POST',
		body: JSON.stringify(volume)
	});
}

export async function removeVolume(id: string, container_path: string): Promise<void> {
	await request(`/apps/${id}/volumes`, {
		method: 'DELETE',
		body: JSON.stringify({ container_path })
	});
}

// ── Auth ──

export async function getAuthStatus(): Promise<AuthStatus> {
	return request<AuthStatus>('/auth/status');
}

/** Create the first admin account. `token` is the one-time setup token from the installer. */
export async function setupAdmin(
	token: string,
	email: string,
	password: string
): Promise<{ user: User }> {
	return request<{ user: User }>('/auth/setup', {
		method: 'POST',
		body: JSON.stringify({ token, email, password })
	});
}

/** Check a setup token before showing the setup form. Throws if it is invalid or used. */
export async function checkSetupToken(token: string): Promise<{ valid: boolean }> {
	return request<{ valid: boolean }>('/auth/setup/check', {
		method: 'POST',
		body: JSON.stringify({ token })
	});
}

export async function login(
	email: string,
	password: string
): Promise<{ user: User } | { mfa_required: true }> {
	return request<{ user: User } | { mfa_required: true }>('/auth/login', {
		method: 'POST',
		body: JSON.stringify({ email, password })
	});
}

export async function logout(): Promise<void> {
	await request('/auth/logout', { method: 'POST' });
}

export async function getMe(): Promise<User> {
	return request<User>('/auth/me');
}

// ── Users (admins: users:admin) ──

export interface AdminUser extends User {
	/** Live (not expired, not idle) sessions */
	active_sessions: number;
	/** Whether this row is the signed-in admin */
	is_you: boolean;
}

/** A one-time set-password link (invite or reset), valid 24 hours. */
export interface PasswordLink {
	user: User;
	/** Dashboard path with the token, e.g. `/set-password?token=…` */
	path: string;
	token: string;
	expires_at: string;
}

export async function listUsers(): Promise<AdminUser[]> {
	return request<AdminUser[]>('/users');
}

/** Invite a user: no password is chosen for them, the response carries a set-password link. */
export async function inviteUser(email: string, role: Role): Promise<PasswordLink> {
	return request<PasswordLink>('/users', {
		method: 'POST',
		body: JSON.stringify({ email, role })
	});
}

/** Change a user's role and/or disable or enable them. Signs them out everywhere. */
export async function updateUser(
	id: string,
	changes: { role?: Role; disabled?: boolean }
): Promise<User> {
	return request<User>(`/users/${id}`, { method: 'PATCH', body: JSON.stringify(changes) });
}

export async function deleteUser(id: string): Promise<void> {
	await request(`/users/${id}`, { method: 'DELETE' });
}

/** New one-time set-password link for a user (replaces older links). */
export async function createPasswordLink(id: string): Promise<PasswordLink> {
	return request<PasswordLink>(`/users/${id}/password-link`, { method: 'POST' });
}

export async function listUserSessions(id: string): Promise<SessionInfo[]> {
	return request<SessionInfo[]>(`/users/${id}/sessions`);
}

export async function revokeUserSessions(id: string): Promise<{ revoked: number }> {
	return request<{ revoked: number }>(`/users/${id}/sessions`, { method: 'DELETE' });
}

export async function revokeUserSession(id: string, sessionId: string): Promise<void> {
	await request(`/users/${id}/sessions/${sessionId}`, { method: 'DELETE' });
}

// ── Set-password links (public) ──

export interface PasswordLinkInfo {
	email: string;
	/** "invite" | "reset" */
	purpose: string;
	expires_at: string;
}

export async function checkPasswordLink(token: string): Promise<PasswordLinkInfo> {
	return request<PasswordLinkInfo>('/auth/password-link/check', {
		method: 'POST',
		body: JSON.stringify({ token })
	});
}

export async function completePasswordLink(
	token: string,
	password: string
): Promise<{ signed_in: boolean; user?: User; message?: string }> {
	return request<{ signed_in: boolean; user?: User; message?: string }>(
		'/auth/password-link/complete',
		{ method: 'POST', body: JSON.stringify({ token, password }) }
	);
}

// ── API tokens ──

export type TokenKind = 'personal' | 'service' | 'agent' | 'ci';

export interface ScopeInfo {
	id: Scope;
	description: string;
}

export interface ScopeGroupInfo {
	id: 'apps' | 'secrets' | 'data' | 'ops' | 'admin';
	label: string;
	scopes: ScopeInfo[];
}

export interface TokenPreset {
	id: string;
	label: string;
	description: string;
	scopes: Scope[];
}

export interface RoleInfo {
	id: Role;
	description: string;
	scopes: Scope[];
}

/** Everything the token form needs, from GET /api/tokens/catalog. */
export interface TokenCatalog {
	groups: ScopeGroupInfo[];
	presets: TokenPreset[];
	roles: RoleInfo[];
	/** Scopes the caller may grant (a token never exceeds its creator) */
	grantable: Scope[];
}

export interface ApiToken {
	id: string;
	name: string;
	/** Visible start of the token, e.g. `tond_pat_1a2b3c4d` */
	prefix: string;
	kind: TokenKind;
	scopes: Scope[];
	app_ids: string[];
	project_ids: string[];
	allowed_ips: string[];
	owner_id: string | null;
	owner_label: string;
	expires_at: string | null;
	last_used_at: string | null;
	last_used_ip: string | null;
	revoked_at: string | null;
	created_at: string;
}

export interface CreateTokenInput {
	name: string;
	kind: TokenKind;
	preset?: string;
	scopes: Scope[];
	app_ids: string[];
	project_ids: string[];
	allowed_ips: string[];
	/** Lifetime in hours; 0 = never expires (not for agent tokens) */
	expires_in_hours: number;
}

export async function getTokenCatalog(): Promise<TokenCatalog> {
	return request<TokenCatalog>('/tokens/catalog');
}

export async function listTokens(all = false): Promise<ApiToken[]> {
	return request<ApiToken[]>(`/tokens${all ? '?all=true' : ''}`);
}

/** Create a token. The secret is only in this response: show it once. */
export async function createToken(
	input: CreateTokenInput
): Promise<{ token: ApiToken; secret: string }> {
	return request<{ token: ApiToken; secret: string }>('/tokens', {
		method: 'POST',
		body: JSON.stringify(input)
	});
}

export async function revokeToken(id: string): Promise<ApiToken> {
	return request<ApiToken>(`/tokens/${id}`, { method: 'DELETE' });
}

// ── Phase 7.1 — Password Management & Session Management ──

export async function changePassword(currentPassword: string, newPassword: string): Promise<void> {
	await request('/auth/change-password', {
		method: 'POST',
		body: JSON.stringify({ current_password: currentPassword, new_password: newPassword })
	});
}

export interface SessionInfo {
	id: string;
	user_agent: string | null;
	ip_address: string | null;
	created_at: string;
	last_used_at: string | null;
	expires_at: string;
	is_current: boolean;
	/** When the session ends if unused (idle timeout) */
	idle_expires_at?: string | null;
}

export async function listSessions(): Promise<SessionInfo[]> {
	return request<SessionInfo[]>('/auth/sessions');
}

export async function revokeSession(sessionId: string): Promise<void> {
	await request(`/auth/sessions/${sessionId}`, { method: 'DELETE' });
}

export async function revokeAllSessions(): Promise<void> {
	await request('/auth/sessions', { method: 'DELETE' });
}

// ── Phase 7.1 — TOTP 2FA ──

export interface TotpSetup {
	secret: string;
	otpauth_uri: string;
	/** Base64-encoded PNG QR code — use as `<img src="data:image/png;base64,{qr_code}">` */
	qr_code: string;
}

/** Start 2FA enrollment. Requires the current password; refused while 2FA is on. */
export async function setup2FA(password: string): Promise<TotpSetup> {
	return request<TotpSetup>('/auth/2fa/setup', {
		method: 'POST',
		body: JSON.stringify({ password })
	});
}

export async function confirm2FA(
	code: string
): Promise<{ message: string; backup_codes: string[] }> {
	return request<{ message: string; backup_codes: string[] }>('/auth/2fa/confirm', {
		method: 'POST',
		body: JSON.stringify({ code })
	});
}

export async function disable2FA(password: string, code: string): Promise<void> {
	await request('/auth/2fa/disable', {
		method: 'POST',
		body: JSON.stringify({ password, code })
	});
}

/** Called after the first login factor when 2FA is enabled. */
export async function verify2FA(code: string): Promise<{ user: User }> {
	return request<{ user: User }>('/auth/2fa/verify', {
		method: 'POST',
		body: JSON.stringify({ code })
	});
}

// ── Settings ──

export interface ServerSettings {
	server_name: string;
	dashboard_domain: string;
	/**
	 * Domain previews are served under (`<slug>-<app>.<preview_domain>`). Must be a
	 * different registrable domain from `dashboard_domain`; empty = previews disabled.
	 */
	preview_domain: string;
	/**
	 * Domain automatic app hostnames live under (`<app>.<apps_domain>`). Must be a
	 * different registrable domain from both the dashboard and preview domains;
	 * empty = apps need a custom domain.
	 */
	apps_domain: string;
	/** Max unpacked size in bytes of an uploaded source archive (`tond push`) */
	upload_max_bytes: number;
	/** Sessions end after this many hours without a request (1–720, default 12) */
	session_idle_timeout_hours?: number;
	/** Sessions end this many days after sign-in (1–365, default 30) */
	session_max_age_days?: number;
}

export interface LastDeployment {
	id: string;
	app_id: string;
	app_name: string;
	status: string;
	trigger_source: string;
	created_at: string;
	finished_at: string | null;
}

export interface OnboardingStatus {
	dashboard_domain: string;
	is_fresh_install: boolean;
	has_projects: boolean;
	has_apps: boolean;
	has_databases: boolean;
	has_cron_jobs: boolean;
	/** A GitHub App is installed, or a GitLab token or registry credential exists. */
	has_source_integration: boolean;
	/** At least one deployment succeeded. */
	has_successful_deploy: boolean;
	/** At least one notification channel exists. */
	has_notification_channel: boolean;
	/** The newest deployment of any app. */
	last_deployment: LastDeployment | null;
	server_public_ip: string | null;
	domain_status: {
		configured: boolean;
		expected_target: string | null;
		dns_matches_server: boolean | null;
		https_ready: boolean | null;
		certificate_status: string | null;
	};
}

export async function getSettings(): Promise<ServerSettings> {
	return request<ServerSettings>('/settings');
}

export async function getOnboardingStatus(): Promise<OnboardingStatus> {
	return request<OnboardingStatus>('/onboarding/status');
}

export async function updateSettings(data: Partial<ServerSettings>): Promise<ServerSettings> {
	return request<ServerSettings>('/settings', {
		method: 'PUT',
		body: JSON.stringify(data)
	});
}

// ── Deployments ──

/** Steps of a deployment, in order ("release" = switch traffic, stop the old container). */
export type DeployPhase = 'clone' | 'build' | 'start' | 'health' | 'release';
export type PhaseStatus = 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped';

export interface PhaseRecord {
	phase: DeployPhase;
	status: PhaseStatus;
	/** This deployment's flavour of the step, e.g. "Pull image" */
	label?: string;
	started_at?: string;
	finished_at?: string;
}

/** The likely cause of a failed deployment, with what to do about it. */
export interface DeployFailure {
	phase?: DeployPhase;
	/** e.g. "build_failed", "port_not_listening", "image_pull_denied" */
	code: string;
	title: string;
	hint: string;
	detail?: string;
	/** Log lines that show the problem */
	excerpt?: string[];
}

export interface Deployment {
	id: string;
	app_id: string;
	image_tag: string | null;
	/**
	 * "running" | "succeeded" | "failed"; a build queue may add others (e.g.
	 * "queued", "superseded") — treat unknown values generically.
	 */
	status: string;
	/** "manual" | "webhook" | "upload" | "rollback" | "cli" | "api" | … */
	trigger_source: string;
	/** Who or what triggered it: a user's email, "GitHub push to main", … */
	trigger_detail?: string | null;
	has_generated_dockerfile: boolean;
	started_at: string;
	finished_at: string | null;
	created_at: string;
	duration_secs?: number | null;
	commit_sha?: string | null;
	commit_message?: string | null;
	commit_author?: string | null;
	/** For rollbacks: the deployment whose image was restored */
	rollback_of?: string | null;
	phases?: PhaseRecord[];
	failure?: DeployFailure | null;
	/** 1-based place in the build queue while status is "queued" */
	queue_position?: number | null;
	/** "deploy" | "rollback" | "upload" */
	kind?: string;
	/** Why it was superseded, or why it failed before running (app deleted, upload lost) */
	error?: string | null;
	/** The deployment that replaced this one while it was queued (status "superseded") */
	superseded_by?: string | null;
	/** The image was removed from the server to save disk space */
	image_pruned: boolean;
	/** This deployment's image is what the app runs now */
	is_current: boolean;
	/** Rolling back to this deployment would restore its exact image */
	rollback_available: boolean;
	/** Why rollback is unavailable (tooltip text); null when available */
	rollback_unavailable_reason: string | null;
}

export interface DeploymentLogLine {
	level: string;
	message: string;
	ts: string;
}

/** A deployment with its log, as returned by GET …/deployments/{id} and the stream snapshot. */
export interface DeploymentDetail extends Deployment {
	logs: DeploymentLogLine[];
	logs_truncated: boolean;
	/** Port Tond expects the app to listen on */
	container_port: number;
	health_check_path: string | null;
	source_type: string;
}

/** 202 body of deploy, rollback and upload requests. */
export interface DeployStarted {
	message: string;
	app_id: string;
	deployment_id: string;
	/** "queued": deploys run one per app, builds share the server's build slots */
	status?: string;
	/** 1-based place in the build queue */
	queue_position?: number | null;
	/** The still-waiting deployment this request replaced, if any */
	superseded?: string | null;
}

export async function listDeployments(id: string): Promise<Deployment[]> {
	return request<Deployment[]>(`/apps/${id}/deployments`);
}

export async function getDeployment(id: string, deploymentId: string): Promise<DeploymentDetail> {
	return request<DeploymentDetail>(`/apps/${id}/deployments/${deploymentId}`);
}

export interface DeploymentStreamCallbacks {
	/** Full state on connect (and on every reconnect). */
	onSnapshot: (detail: DeploymentDetail) => void;
	onLog: (line: DeploymentLogLine) => void;
	/** The deployment row changed (phase, commit, result). */
	onDeployment: (deployment: Deployment) => void;
	/** Connected (again); a snapshot follows. */
	onOpen?: (reconnected: boolean) => void;
	/** The connection dropped; it reconnects by itself in `retryInMs`. */
	onDisconnect?: (retryInMs: number) => void;
}

/** Statuses after which a deployment no longer changes. */
const FINISHED_DEPLOYMENT = ['succeeded', 'failed', 'superseded', 'cancelled', 'canceled'];

/**
 * Live deploy view stream. Reconnects after a drop (each connection starts
 * with a full snapshot, so nothing is missed) and closes for good once the
 * deployment has finished. Returns a `close()` function for $effect cleanup.
 * See handlers/deployments.rs `stream_deployment` for the frames.
 */
export function openDeploymentStream(
	id: string,
	deploymentId: string,
	callbacks: DeploymentStreamCallbacks
): () => void {
	const socket: ReconnectingSocket = connectWithRetry(
		() => wsUrl(`${BASE}/apps/${id}/deployments/${deploymentId}/stream`),
		{
			onOpen: (reconnected) => callbacks.onOpen?.(reconnected),
			onDisconnect: (retryInMs) => callbacks.onDisconnect?.(retryInMs),
			onMessage: (ev) => {
				let frame: any;
				try {
					frame = JSON.parse(ev.data as string);
				} catch {
					return;
				}
				let status: string | undefined;
				if (frame.type === 'snapshot') {
					status = frame.deployment?.status;
					callbacks.onSnapshot({
						...frame.deployment,
						logs: frame.logs ?? [],
						logs_truncated: !!frame.logs_truncated,
						container_port: frame.container_port,
						health_check_path: frame.health_check_path ?? null,
						source_type: frame.source_type ?? ''
					});
				} else if (frame.type === 'log') {
					callbacks.onLog({ level: frame.level, message: frame.message, ts: frame.ts });
				} else if (frame.type === 'deployment') {
					status = frame.deployment?.status;
					callbacks.onDeployment(frame.deployment);
				}
				// Finished: the server closes the socket; don't reconnect.
				if (status && FINISHED_DEPLOYMENT.includes(status)) socket.close();
			}
		}
	);
	return () => socket.close();
}

export async function getDeploymentGeneratedDockerfile(
	id: string,
	deploymentId: string
): Promise<string> {
	const response = await request<{ deployment_id: string; generated_dockerfile: string }>(
		`/apps/${id}/deployments/${deploymentId}/generated-dockerfile`
	);
	return response.generated_dockerfile;
}

/**
 * Redeploy the exact image of an earlier deployment. Without `deploymentId`
 * the server restores the previous distinct image.
 */
export async function rollbackApp(id: string, deploymentId?: string): Promise<DeployStarted> {
	return request<DeployStarted>(`/apps/${id}/rollback`, {
		method: 'POST',
		...(deploymentId ? { body: JSON.stringify({ deployment_id: deploymentId }) } : {})
	});
}

// ── Source inspection (new-app wizard) ──

export interface RemoteBranches {
	default_branch: string | null;
	/** Default branch first */
	branches: string[];
	truncated: boolean;
}

/** Branches of a Git repository (http/https URLs only). */
export async function listRemoteBranches(
	url: string,
	opts: { github_app_id?: string; gitlab_token_id?: string } = {}
): Promise<RemoteBranches> {
	return request<RemoteBranches>('/git/branches', {
		method: 'POST',
		body: JSON.stringify({ url, ...opts })
	});
}

export interface ImageInspection {
	image: string;
	exposed_ports: number[];
	env_port: number | null;
	suggested_port: number | null;
	pulled: boolean;
	/** Set when the image couldn't be read; says why and what to do */
	unavailable?: string | null;
	/** The registry refused the image: it may need registry credentials */
	needs_credentials?: boolean;
}

/** Ports an image exposes (pulls it first if it isn't on the server yet). */
export async function inspectImage(
	image: string,
	registryCredentialId?: string
): Promise<ImageInspection> {
	return request<ImageInspection>('/images/inspect', {
		method: 'POST',
		body: JSON.stringify({
			image,
			...(registryCredentialId ? { registry_credential_id: registryCredentialId } : {})
		})
	});
}

// ── Domain checks ──

export interface DnsRecord {
	type: 'A' | 'AAAA';
	/** Name relative to the zone: "app", "@", "*.apps" */
	name: string;
	host: string;
	value: string;
}

export type DnsCheckStatus = 'ok' | 'mismatch' | 'missing' | 'inconclusive';

export interface DomainCertificate {
	status: string;
	issued_at: string;
	expires_at: string;
	last_error: string | null;
	last_attempt_at: string | null;
	next_retry_at: string | null;
	failure_count: number;
}

export interface DomainCheck {
	domain: string;
	zone: string | null;
	apps_domain: string | null;
	under_apps_domain: boolean;
	records: DnsRecord[];
	wildcard_records: DnsRecord[];
	server_ips: string[];
	dns: {
		status: DnsCheckStatus;
		resolved: string[];
		expected: string[];
		stray: string[];
		message: string;
	};
	certificate: DomainCertificate | null;
	checked_at: string;
	/** Set when the domain is on the same site as the dashboard (allowed, shown as a warning) */
	warning?: string | null;
}

/** Live DNS check: the records to create, what the domain resolves to, and its certificate. */
export async function checkDomain(domain: string): Promise<DomainCheck> {
	return request<DomainCheck>(`/domains/check?domain=${encodeURIComponent(domain)}`);
}

// ── Git Integration ──

export interface DeployKey {
	app_id: string;
	public_key: string;
}

export async function generateDeployKey(id: string): Promise<DeployKey> {
	return request<DeployKey>(`/apps/${id}/deploy-key`, { method: 'POST' });
}

export async function getDeployKey(id: string): Promise<DeployKey> {
	return request<DeployKey>(`/apps/${id}/deploy-key`);
}

export interface WebhookUrls {
	github: string;
	gitlab: string;
	gitea: string;
	generic: string;
}

export interface WebhookConfig {
	app_id: string;
	/** Paths relative to the server origin, e.g. `/api/webhooks/github/<uuid>` */
	paths: WebhookUrls;
	/** Absolute URLs when the dashboard domain is configured, else null */
	urls: WebhookUrls | null;
	/**
	 * Whether a shared secret (HMAC key for GitHub/Gitea/generic, token for
	 * GitLab) is set. The secret is write-only: see `revealWebhookSecret`.
	 */
	has_secret: boolean;
	auto_deploy: boolean;
	watch_branch: string | null;
}

export interface WebhookSecret {
	secret: string;
}

/** Reveal the webhook secret (admin only; every reveal is audit-logged). */
export async function revealWebhookSecret(id: string): Promise<WebhookSecret> {
	return request<WebhookSecret>(`/apps/${id}/webhook-secret`);
}

export async function getWebhookConfig(id: string): Promise<WebhookConfig> {
	return request<WebhookConfig>(`/apps/${id}/webhook`);
}

/** Replace the webhook secret with a custom value (min. 16 characters). */
export async function setWebhookSecret(id: string, secret: string): Promise<WebhookConfig> {
	return request<WebhookConfig>(`/apps/${id}/webhook-secret`, {
		method: 'PUT',
		body: JSON.stringify({ secret })
	});
}

/**
 * Generate a new webhook secret; the old one stops working immediately. The
 * new secret is returned once, in this response.
 */
export async function rotateWebhookSecret(id: string): Promise<WebhookSecret> {
	return request<WebhookSecret>(`/apps/${id}/webhook-secret/rotate`, { method: 'POST' });
}

export async function setAutoDeploy(
	id: string,
	auto_deploy: boolean,
	watch_branch: string | null
): Promise<App> {
	return request<App>(`/apps/${id}/auto-deploy`, {
		method: 'PUT',
		body: JSON.stringify({ auto_deploy, watch_branch })
	});
}

export async function updateSource(
	id: string,
	source_type: string,
	source_url: string | null
): Promise<App> {
	return request<App>(`/apps/${id}/source`, {
		method: 'PUT',
		body: JSON.stringify({ source_type, source_url })
	});
}

export async function setAppRegistryCredential(
	id: string,
	registry_credential_id: string | null
): Promise<App> {
	return request<App>(`/apps/${id}/registry-credential`, {
		method: 'PUT',
		body: JSON.stringify({ registry_credential_id })
	});
}

// ── GitHub App (Phase 4.4) ──

export interface GitHubApp {
	id: string;
	name: string;
	app_id: number;
	client_id: string;
	slug: string;
	/** Numeric installation ID from GitHub; null until the App is installed on repos */
	installation_id: number | null;
	/** True when installation_id is set — pre-computed by the server */
	installed: boolean;
	created_at: string;
	updated_at: string;
}

export async function listGitHubApps(): Promise<GitHubApp[]> {
	return request<GitHubApp[]>('/github-apps');
}

export interface GitHubAppManifestForm {
	/** POST action target: https://github.com/settings/apps/new?state=<uuid> */
	github_url: string;
	/** JSON string to place in the hidden <input name="manifest"> field */
	manifest: string;
}

export async function getGitHubAppManifestUrl(): Promise<GitHubAppManifestForm> {
	return request<GitHubAppManifestForm>('/github-apps/manifest', { method: 'POST' });
}

/**
 * Start an installation flow for the logged-in admin and return the GitHub
 * installation URL (it carries a single-use state; do not cache it).
 */
export async function getGitHubAppInstallUrl(id: string): Promise<string> {
	const res = await request<{ url: string }>(`/github-apps/${id}/install`, { method: 'POST' });
	return res.url;
}

export interface CompleteGitHubFlowRequest {
	kind: 'manifest' | 'install';
	state: string | null;
	code?: string | null;
	installation_id?: number | null;
}

/**
 * Finish a GitHub App manifest or install flow after GitHub redirects back.
 * Rejects with an error whose message is a flow error code (e.g. `invalid_state`).
 */
export async function completeGitHubFlow(req: CompleteGitHubFlowRequest): Promise<void> {
	await request('/github-apps/complete', { method: 'POST', body: JSON.stringify(req) });
}

export async function deleteGitHubApp(id: string): Promise<void> {
	await request(`/github-apps/${id}`, { method: 'DELETE' });
}

export async function updateGitHubApp(
	id: string,
	payload: { name?: string; installation_id?: number }
): Promise<GitHubApp> {
	return request<GitHubApp>(`/github-apps/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(payload)
	});
}

export interface GitHubRepo {
	id: number;
	full_name: string;
	name: string;
	private: boolean;
	clone_url: string;
	ssh_url: string;
	default_branch: string;
}

export async function listGitHubRepos(id: string): Promise<GitHubRepo[]> {
	return request<GitHubRepo[]>(`/github-apps/${id}/repos`);
}

export interface GitHubBranch {
	name: string;
}

export async function listGitHubBranches(
	id: string,
	owner: string,
	repo: string
): Promise<GitHubBranch[]> {
	return request<GitHubBranch[]>(`/github-apps/${id}/repos/${owner}/${repo}/branches`);
}

export async function setAppGitHubApp(appId: string, githubAppId: string | null): Promise<App> {
	return request<App>(`/apps/${appId}/github-app`, {
		method: 'PUT',
		body: JSON.stringify({ github_app_id: githubAppId })
	});
}

// ── Registry Credentials ──

export interface RegistryCredential {
	id: string;
	name: string;
	server: string;
	username: string;
	created_at: string;
	updated_at: string;
}

export async function listRegistryCredentials(): Promise<RegistryCredential[]> {
	return request<RegistryCredential[]>('/registry-credentials');
}

export async function createRegistryCredential(data: {
	name: string;
	server: string;
	username: string;
	password: string;
}): Promise<RegistryCredential> {
	return request<RegistryCredential>('/registry-credentials', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function deleteRegistryCredential(id: string): Promise<void> {
	await request<void>(`/registry-credentials/${id}`, { method: 'DELETE' });
}

// ── Preview Deployments ──

export interface PreviewDeployment {
	id: string;
	app_id: string;
	project_preview_id: string | null;
	slug: string;
	subdomain: string | null;
	container_id: string | null;
	host_port: number | null;
	image_tag: string | null;
	ttl_seconds: number;
	status: string;
	expires_at: string;
	created_at: string;
	url: string | null;
	// Phase 4.6: PR preview fields
	pr_number: number | null;
	head_sha: string | null;
	head_ref: string | null;
	pr_url: string | null;
	comment_id: number | null;
}

export interface GithubPR {
	number: number;
	title: string;
	head_sha: string;
	head_ref: string;
	pr_url: string;
}

export async function listOpenPRs(appId: string): Promise<GithubPR[]> {
	return request<GithubPR[]>(`/apps/${appId}/github-prs`);
}

export async function createPreview(
	id: string,
	options: {
		ttl_hours?: number;
		pr_number?: number;
		head_sha?: string;
		head_ref?: string;
		pr_url?: string;
	} = {}
): Promise<PreviewDeployment> {
	return request<PreviewDeployment>(`/apps/${id}/preview`, {
		method: 'POST',
		body: JSON.stringify({ ttl_hours: 24, ...options })
	});
}

export async function listPreviews(id: string): Promise<PreviewDeployment[]> {
	return request<PreviewDeployment[]>(`/apps/${id}/previews`);
}

export async function deletePreview(id: string, previewId: string): Promise<void> {
	await request(`/apps/${id}/previews/${previewId}`, { method: 'DELETE' });
}

/** Enable or disable PR previews. Preview env is edited with patchPreviewEnv(). */
export async function updatePreviewConfig(id: string, preview_enabled: boolean): Promise<void> {
	await request(`/apps/${id}/preview-config`, {
		method: 'PUT',
		body: JSON.stringify({ preview_enabled })
	});
}

// ── Metrics ──

export type MetricsRange = '1h' | '24h' | '7d' | '30d';

/** Downsampled series as parallel arrays (one entry per bucket). */
export interface MetricSeries {
	/** Bucket start, unix seconds. */
	t: number[];
	/** Average CPU, percent of one core (100 = one full core). */
	cpu: number[];
	cpu_max: number[];
	/** Average working-set memory in bytes. */
	mem: number[];
	mem_max: number[];
	/** Bytes per second. */
	rx: number[];
	tx: number[];
	pids: number[];
}

export type MetricEventKind = 'restart' | 'crash' | 'oom';

export interface MetricEvent {
	/** Unix seconds. */
	t: number;
	kind: MetricEventKind;
	detail: string | null;
}

export interface MetricCurrent {
	t: number;
	cpu: number;
	mem: number;
	mem_peak: number | null;
	rx: number;
	tx: number;
	pids: number;
}

export interface MetricsResponse {
	range: MetricsRange;
	/** Bucket width in seconds. */
	step: number;
	from: number;
	to: number;
	/** Memory limit in bytes, null when unlimited. */
	mem_limit: number | null;
	series: MetricSeries;
	events: MetricEvent[];
	current: MetricCurrent | null;
}

export interface SystemMetrics {
	cpu_percent: number;
	mem_used: number;
	mem_total: number;
	mem_percent: number;
	/** Filesystem holding Tond's data directory (what the disk alert watches). */
	disk_used: number;
	disk_total: number;
	disk_percent: number;
	disk_mount: string;
	/** Running containers Tond manages. */
	containers_running: number;
	apps_total: number;
	apps_running: number;
	collected_at: string;
}

export type MetricsOwner = 'apps' | 'databases' | 'cron';

/** Resource metrics of an app, database or cron job. */
export async function getMetrics(
	owner: MetricsOwner,
	id: string,
	range: MetricsRange = '1h'
): Promise<MetricsResponse> {
	return request<MetricsResponse>(
		`/${owner}/${encodeURIComponent(id)}/metrics?range=${encodeURIComponent(range)}`
	);
}

export async function getSystemMetrics(): Promise<SystemMetrics> {
	return request<SystemMetrics>('/system/metrics');
}

// ── Tond measuring itself (GET /api/system/self) ──

/** Health of one background loop of tond-server or tond-proxy. */
export interface LoopStats {
	name: string;
	/** Times the loop woke up since the process started */
	wakeups: number;
	runs: number;
	errors: number;
	last_run_at: string | null;
	last_duration_ms: number | null;
	/** Most recent failure; cleared by a later success */
	last_error: string | null;
	/** Largest delay of a timer past its deadline, ms */
	max_lag_ms: number;
	/** Next timed wake-up; null while waiting only for an event */
	next_wake_at: string | null;
}

export interface ProcessSample {
	rss_bytes: number | null;
	cpu_seconds: number | null;
	open_fds: number | null;
	threads: number | null;
	context_switches: number | null;
}

export interface ServerProcessStats extends ProcessSample {
	cpu_percent: number | null;
	uptime_secs: number;
	tokio_alive_tasks: number;
	tokio_workers: number;
	tokio_parks: number;
}

export interface ProxyStats extends ProcessSample {
	pid: number;
	uptime_secs: number;
	route_version: number;
	routes: number;
	reloads: number;
	loops: LoopStats[];
}

export interface SystemSelf {
	version: string;
	server: ServerProcessStats;
	loops: LoopStats[];
	db: {
		size_bytes: number | null;
		wal_bytes: number | null;
		queries_total: number;
		slow_total: number;
		slow_threshold_ms: number;
		recent_slow: { sql: string; elapsed_ms: number; at: string }[];
		top_statements: { sql: string; count: number }[];
	};
	http_bucket_bounds_ms: number[];
	http: {
		method: string;
		route: string;
		count: number;
		errors: number;
		sum_ms: number;
		buckets: number[];
		p50_ms: number;
		p95_ms: number;
	}[];
	build_queue: { build_slots: number; active: number; queued: number };
	proxy: { connected: boolean; stats: ProxyStats | null; cpu_percent: number | null };
}

export async function getSystemSelf(): Promise<SystemSelf> {
	return request<SystemSelf>('/system/self');
}

// ── Alerts ──

/** Rule kinds (one condition per rule). */
export type AlertKind =
	| 'app_down'
	| 'crash_loop'
	| 'oom'
	| 'cpu'
	| 'memory'
	| 'http_5xx_rate'
	| 'http_p95_latency'
	| 'deploy_failed'
	| 'disk_usage'
	| 'disk_forecast'
	| 'cert_expiry'
	| 'backup_failed'
	| 'cron_failed';

export type AlertSeverity = 'info' | 'warning' | 'critical';
/** What a rule's targets are. */
export type AlertScope = 'app' | 'host' | 'certificate' | 'backup' | 'cron';
/** How the value is compared with the threshold ("none": a yes/no condition). */
export type AlertComparison = 'above' | 'at_least' | 'below' | 'none';

/** One rule kind as described by the server (GET /api/alerts/kinds). */
export interface AlertKindInfo {
	kind: AlertKind;
	label: string;
	description: string;
	scope: AlertScope;
	/** Unit of the threshold and value: "%", "ms", "days", "hours", "restarts" or "" */
	unit: string;
	comparison: AlertComparison;
	default_threshold: number | null;
	default_window_secs: number | null;
	default_for_secs: number;
	default_severity: AlertSeverity;
	/** False while this server can't provide the data (e.g. request metrics) */
	available: boolean;
	unavailable_reason: string | null;
}

export interface AlertRule {
	id: string;
	name: string;
	kind: AlertKind;
	/** App-scoped kinds: one app, or null for every app */
	app_id: string | null;
	app_name: string | null;
	threshold: number;
	/** Look-back window in seconds (5xx rate, p95, OOM, crash loop) */
	window_secs: number;
	/** How long the condition must hold before the alert fires */
	for_secs: number;
	severity: AlertSeverity;
	/** Re-notify while firing and unacknowledged; 0 = never */
	repeat_secs: number;
	enabled: boolean;
	/** The whole rule is muted until then */
	snoozed_until: string | null;
	/** Linked notification channels (at least one) */
	integration_ids: string[];
	/** Targets firing / pending right now */
	firing: number;
	pending: number;
	last_fired_at: string | null;
	created_at: string;
	updated_at: string;
}

export interface AlertRuleInput {
	name: string;
	kind: AlertKind;
	app_id?: string | null;
	threshold?: number;
	window_secs?: number;
	for_secs?: number;
	severity?: AlertSeverity;
	repeat_secs?: number;
	integration_ids: string[];
}

export interface AlertRuleUpdate {
	name?: string;
	threshold?: number;
	window_secs?: number;
	for_secs?: number;
	severity?: AlertSeverity;
	repeat_secs?: number;
	enabled?: boolean;
	integration_ids?: string[];
	/** RFC 3339 to mute the whole rule, "" to unmute */
	snoozed_until?: string;
}

/** A pending or firing alert. */
export interface ActiveAlert {
	rule_id: string;
	rule_name: string;
	kind: AlertKind;
	severity: AlertSeverity;
	/** "app:<id>" | "host" | "cert:<domain>" | "backup" | "cron:<id>" */
	target: string;
	target_label: string;
	app_id: string | null;
	state: 'pending' | 'firing';
	/** When the condition became true */
	since: string;
	fired_at: string | null;
	value: number | null;
	threshold: number;
	unit: string;
	comparison: AlertComparison;
	/** History row of the firing episode */
	episode_id: string | null;
	acknowledged_at: string | null;
	acknowledged_by: string | null;
	snoozed_until: string | null;
	snoozed_until_resolved: boolean;
	/** Notifications of this alert that failed for good */
	failed_deliveries: number;
}

/** One firing episode in the history. */
export interface AlertEpisode {
	id: string;
	rule_id: string | null;
	rule_name: string;
	kind: AlertKind;
	severity: AlertSeverity;
	target: string;
	target_label: string;
	app_id: string | null;
	threshold: number;
	unit: string;
	comparison: AlertComparison;
	/** Value when it fired, and the latest one seen */
	value: number | null;
	last_value: number | null;
	state: 'firing' | 'resolved';
	pending_since: string;
	fired_at: string;
	resolved_at: string | null;
	acknowledged_at: string | null;
	acknowledged_by: string | null;
	notifications_sent: number;
	failed_deliveries: number;
}

export interface AlertHistoryPage {
	episodes: AlertEpisode[];
	total: number;
}

export interface AlertHistoryFilter {
	rule_id?: string;
	app_id?: string;
	state?: 'firing' | 'resolved';
	page?: number;
	per_page?: number;
}

/** One notification to one channel. */
export interface AlertDelivery {
	id: string;
	episode_id: string;
	integration_id: string;
	integration_name: string;
	integration_kind: IntegrationKind;
	event: 'firing' | 'repeat' | 'resolved';
	status: 'pending' | 'sent' | 'failed' | 'skipped';
	attempts: number;
	/** Held back by quiet hours */
	deferred: boolean;
	last_error: string | null;
	next_attempt_at: string | null;
	sent_at: string | null;
	created_at: string;
}

export interface AlertEpisodeDetail {
	episode: AlertEpisode;
	deliveries: AlertDelivery[];
}

/** A daily window when notifications wait (critical ones can bypass it). */
export interface QuietHours {
	enabled: boolean;
	/** "HH:MM" */
	start: string;
	end: string;
	/** IANA time zone, e.g. "Europe/Berlin" */
	timezone: string;
	critical_bypass: boolean;
}

/** A channel follows the global quiet hours, has none, or its own. */
export type ChannelQuietHours =
	{ mode: 'global' } | { mode: 'off' } | ({ mode: 'custom' } & QuietHours);

export interface AlertSettings {
	quiet_hours: QuietHours;
}

export async function listAlertKinds(): Promise<AlertKindInfo[]> {
	return request<AlertKindInfo[]>('/alerts/kinds');
}

export async function listAlertRules(): Promise<AlertRule[]> {
	return request<AlertRule[]>('/alerts');
}

export async function getAlertRule(id: string): Promise<AlertRule> {
	return request<AlertRule>(`/alerts/${id}`);
}

export async function createAlertRule(data: AlertRuleInput): Promise<AlertRule> {
	return request<AlertRule>('/alerts', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function updateAlertRule(id: string, data: AlertRuleUpdate): Promise<AlertRule> {
	return request<AlertRule>(`/alerts/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

export async function deleteAlertRule(id: string): Promise<void> {
	await request(`/alerts/${id}`, { method: 'DELETE' });
}

export interface AlertChannelTestResult {
	integration_id: string;
	/** The channel kind ("slack", "ntfy", "webhook", …) */
	kind: string;
	name: string;
	ok: boolean;
	error: string | null;
}

/** Send a test message to every channel of a rule; one result per channel. */
export async function testAlertRule(id: string): Promise<AlertChannelTestResult[]> {
	return request<AlertChannelTestResult[]>(`/alerts/${id}/test`, { method: 'POST' });
}

/** Pending and firing alerts, firing first. */
export async function listActiveAlerts(): Promise<ActiveAlert[]> {
	return request<ActiveAlert[]>('/alerts/active');
}

export async function listAlertHistory(filter: AlertHistoryFilter = {}): Promise<AlertHistoryPage> {
	const params = new URLSearchParams();
	for (const [key, value] of Object.entries(filter)) {
		if (value !== undefined && value !== '') params.set(key, String(value));
	}
	const qs = params.toString();
	return request<AlertHistoryPage>(`/alerts/history${qs ? `?${qs}` : ''}`);
}

export async function getAlertEpisode(id: string): Promise<AlertEpisodeDetail> {
	return request<AlertEpisodeDetail>(`/alerts/history/${id}`);
}

/** Stop repeat notifications for a firing alert. */
export async function acknowledgeAlert(episodeId: string): Promise<void> {
	await request(`/alerts/history/${episodeId}/acknowledge`, { method: 'POST' });
}

/** Mute one target of a rule (or the whole rule without `target`). */
export async function snoozeAlert(
	ruleId: string,
	snooze: { target?: string; minutes?: number; until_resolved?: boolean }
): Promise<void> {
	await request(`/alerts/${ruleId}/snooze`, {
		method: 'POST',
		body: JSON.stringify(snooze)
	});
}

export async function unsnoozeAlert(ruleId: string, target?: string): Promise<void> {
	await request(`/alerts/${ruleId}/unsnooze`, {
		method: 'POST',
		body: JSON.stringify(target ? { target } : {})
	});
}

export async function getAlertSettings(): Promise<AlertSettings> {
	return request<AlertSettings>('/alerts/settings');
}

export async function updateAlertSettings(settings: AlertSettings): Promise<AlertSettings> {
	return request<AlertSettings>('/alerts/settings', {
		method: 'PUT',
		body: JSON.stringify(settings)
	});
}

// ── Audit log ──

/** One entry of the append-only, hash-chained audit log. */
export interface AuditLogEntry {
	/** Position in the chain; also the pagination cursor */
	seq: number;
	created_at: string;
	/** "user" | "token" | "local_socket" | "system" | "anonymous" */
	actor_kind: string;
	actor_id: string | null;
	/** Email, token name, "local-socket", … */
	actor_label: string;
	action: string;
	/** "app" | "database" | "user" | "token" | "settings" | … */
	resource_type: string | null;
	resource_id: string | null;
	resource_name: string | null;
	/** "success" | "failure" */
	outcome: string;
	ip: string | null;
	detail: string | null;
}

export interface AuditLogPage {
	entries: AuditLogEntry[];
	/** Pass as `before` to load the next (older) page; null on the last page */
	next_before: number | null;
}

export interface AuditLogQuery {
	actor?: string;
	/** Exact action, or a prefix ending in `*` (e.g. `login*`) */
	action?: string;
	resource_type?: string;
	resource_id?: string;
	/** Free-text search over actor, action, resource and detail */
	q?: string;
	outcome?: 'success' | 'failure';
	before?: number;
	limit?: number;
}

/** Filtered, paginated audit log (filters run on the server, before the limit). */
export async function listAuditLog(params: AuditLogQuery = {}): Promise<AuditLogPage> {
	const qs = new URLSearchParams();
	for (const [key, value] of Object.entries(params)) {
		if (value !== undefined && value !== null && value !== '') qs.set(key, String(value));
	}
	const query = qs.toString() ? `?${qs.toString()}` : '';
	return request<AuditLogPage>(`/audit-log${query}`);
}

export interface AuditVerifyReport {
	ok: boolean;
	checked: number;
	checkpoint_seq: number;
	first_seq: number | null;
	last_seq: number | null;
	broken_at: number | null;
	problem: string | null;
}

/** Walk the audit log's hash chain on the server. */
export async function verifyAuditLog(): Promise<AuditVerifyReport> {
	return request<AuditVerifyReport>('/audit-log/verify');
}

// ── Projects (Phase 6.1) ──

export interface Project {
	id: string;
	name: string;
	description: string | null;
	/** Shared env var keys; values are masked (write-only) */
	env: EnvVarEntry[];
	created_at: string;
	updated_at: string;
	/** Number of active (non-expired) project previews. Returned by GET /projects. */
	active_preview_count: number;
}

export interface ProjectDetail extends Project {
	apps: App[];
}

export async function listProjects(): Promise<Project[]> {
	return request<Project[]>('/projects');
}

export async function getProject(id: string): Promise<ProjectDetail> {
	return request<ProjectDetail>(`/projects/${id}`);
}

export async function createProject(data: {
	name: string;
	description?: string;
	env_vars?: Record<string, string>;
}): Promise<Project> {
	return request<Project>('/projects', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function updateProject(
	id: string,
	data: Partial<{
		name: string;
		description: string;
		env_vars: Record<string, string>;
	}>
): Promise<Project> {
	return request<Project>(`/projects/${id}`, {
		method: 'PUT',
		body: JSON.stringify(data)
	});
}

export async function deleteProject(id: string): Promise<void> {
	await request(`/projects/${id}`, { method: 'DELETE' });
}

export async function deployProject(id: string): Promise<void> {
	await request(`/projects/${id}/deploy`, { method: 'POST' });
}

export async function assignAppProject(appId: string, projectId: string | null): Promise<void> {
	await request(`/apps/${appId}/project`, {
		method: 'PUT',
		body: JSON.stringify({ project_id: projectId })
	});
}

// ── Project Previews (Phase 6.2) ──

/** Aggregated preview environment for a whole project (one per member app). */
export interface ProjectPreview {
	id: string;
	project_id: string;
	/** Shared slug prefix: e.g. "abc123" → abc123-frontend.domain, abc123-api.domain */
	slug: string;
	ttl_seconds: number;
	/** Computed from child previews: "building" | "running" | "partially_failed" | "failed" | "expired" */
	status: string;
	expires_at: string;
	created_at: string;
	/** Per-app preview deployments linked to this project preview */
	previews: PreviewDeployment[];
}

export async function createProjectPreview(
	projectId: string,
	ttl_hours: number = 24
): Promise<ProjectPreview> {
	return request<ProjectPreview>(`/projects/${projectId}/previews`, {
		method: 'POST',
		body: JSON.stringify({ ttl_hours })
	});
}

export async function listProjectPreviews(projectId: string): Promise<ProjectPreview[]> {
	return request<ProjectPreview[]>(`/projects/${projectId}/previews`);
}

export async function deleteProjectPreview(projectId: string, previewId: string): Promise<void> {
	await request(`/projects/${projectId}/previews/${previewId}`, { method: 'DELETE' });
}

// ── Managed Databases (Phase 6.3) ──

export interface ManagedDatabase {
	id: string;
	name: string;
	db_type: string;
	project_id: string | null;
	host_port: number | null;
	status: string;
	db_name: string | null;
	db_user: string | null;
	/** Whether the database has an application password (never returned) */
	has_password: boolean;
	/** Whether there is a separate root/admin password (MySQL and MariaDB) */
	has_root_password: boolean;
	memory_limit: number | null;
	/** Connection-string override, password masked */
	custom_external_url: string | null;
	/** Connection-string override, password masked */
	custom_internal_url: string | null;
	/** Docker image actually used when the container was last created/redeployed */
	image_used: string | null;
	/** User-supplied image override for the next redeploy */
	custom_image: string | null;
	created_at: string;
	updated_at: string;
	/** Port published on all interfaces (reachable from the internet). Off by default. */
	public_access: boolean;
}

/** Connection strings in a detail response have the password masked. */
export interface ManagedDatabaseDetail extends ManagedDatabase {
	connection_string: string | null;
	/** Connection string via the server's public address; only when public_access is on */
	public_connection_string: string | null;
	internal_connection_string: string | null;
}

// ── Database access (links) ──
// Every app and database runs on its own Docker network unless they share a
// project. An app outside the database's project reaches it only through a
// link, which attaches the database to that app's network.

export interface DatabaseAppRef {
	id: string;
	name: string;
	status: string;
	project_id: string | null;
}

export interface DatabaseAccess {
	database_id: string;
	project: { id: string; name: string } | null;
	/** Apps that reach the database through the shared project network */
	project_apps: DatabaseAppRef[];
	/** Apps linked to the database */
	linked_apps: DatabaseAppRef[];
	/** Hostname apps use (the database's name) */
	internal_host: string;
	internal_port: number;
}

export interface AppDatabaseAccess {
	id: string;
	name: string;
	db_type: string;
	status: string;
	/** "project": same project network; "link": linked to the app */
	via: 'project' | 'link';
	internal_host: string;
	internal_port: number;
}

export async function getDatabaseAccess(id: string): Promise<DatabaseAccess> {
	return request<DatabaseAccess>(`/databases/${id}/access`);
}

/** Link an app to a database: the database joins the app's network. Idempotent. */
export async function linkDatabaseApp(databaseId: string, appId: string): Promise<DatabaseAccess> {
	return request<DatabaseAccess>(`/databases/${databaseId}/links/${appId}`, { method: 'PUT' });
}

export async function unlinkDatabaseApp(
	databaseId: string,
	appId: string
): Promise<DatabaseAccess> {
	return request<DatabaseAccess>(`/databases/${databaseId}/links/${appId}`, { method: 'DELETE' });
}

/** Databases an app can reach (through its project or a link). */
export async function listAppDatabases(appId: string): Promise<AppDatabaseAccess[]> {
	return request<AppDatabaseAccess[]>(`/apps/${appId}/databases`);
}

/** Plaintext credentials: GET /databases/{id}/credentials (admin, audit-logged). */
export interface DatabaseCredentials {
	db_user: string | null;
	db_password: string | null;
	root_password: string | null;
	connection_string: string | null;
	public_connection_string: string | null;
	internal_connection_string: string | null;
}

/** POST /databases: the detail plus the generated credentials, shown once. */
export interface CreatedDatabase extends ManagedDatabaseDetail {
	credentials: DatabaseCredentials;
}

/** Reveal a database's passwords and connection strings (admin only; audit-logged). */
export async function revealDatabaseCredentials(id: string): Promise<DatabaseCredentials> {
	return request<DatabaseCredentials>(`/databases/${id}/credentials`);
}

/** A database's own backups plus the full snapshots (which hold every database). */
export async function listDatabaseBackups(id: string): Promise<Snapshot[]> {
	return request<Snapshot[]>(`/databases/${id}/backups`);
}

/** Back up one database now. Resolves when the local copy is complete. */
export async function createDatabaseBackup(id: string): Promise<Snapshot> {
	return request<Snapshot>(`/databases/${id}/backups`, { method: 'POST' });
}

export async function listDatabases(): Promise<ManagedDatabase[]> {
	return request<ManagedDatabase[]>('/databases');
}

export async function getDatabase(id: string): Promise<ManagedDatabaseDetail> {
	return request<ManagedDatabaseDetail>(`/databases/${id}`);
}

export async function createDatabase(data: {
	name: string;
	db_type: string;
	project_id?: string;
	memory_limit?: number;
}): Promise<CreatedDatabase> {
	return request<CreatedDatabase>('/databases', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function deleteDatabase(id: string): Promise<void> {
	await request(`/databases/${id}`, { method: 'DELETE' });
}

export async function stopDatabase(id: string): Promise<void> {
	await request(`/databases/${id}/stop`, { method: 'POST' });
}

export async function startDatabase(id: string): Promise<void> {
	await request(`/databases/${id}/start`, { method: 'POST' });
}

export async function updateDatabase(
	id: string,
	data: {
		custom_external_url?: string | null;
		custom_internal_url?: string | null;
		custom_image?: string | null;
		memory_limit?: number | null;
		/** Admin only. The container is recreated immediately. */
		public_access?: boolean;
	}
): Promise<ManagedDatabaseDetail> {
	return request<ManagedDatabaseDetail>(`/databases/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

export async function redeployDatabase(id: string): Promise<ManagedDatabaseDetail> {
	return request<ManagedDatabaseDetail>(`/databases/${id}/redeploy`, { method: 'POST' });
}

export interface UpdateDatabaseCredentialsResponse {
	database: ManagedDatabaseDetail;
	/** Fields changed inside the running database and saved, e.g. ["db_password"] */
	changed: ('db_password' | 'root_password')[];
}

/**
 * Change a password inside the running database (the server runs ALTER USER /
 * ALTER ROLE in the container), then save it. Refused when the database is
 * stopped, for Redis/KeyDB, and for database or user renames.
 */
export async function updateDatabaseCredentials(
	id: string,
	data: {
		db_password?: string | null;
		root_password?: string | null;
	}
): Promise<UpdateDatabaseCredentialsResponse> {
	return request<UpdateDatabaseCredentialsResponse>(`/databases/${id}/credentials`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

// ── Cron Jobs (Phase 6.4 + 6.5) ──

export interface CronJob {
	id: string;
	name: string;
	schedule: string;
	image: string;
	command: string | null;
	project_id: string | null;
	enabled: boolean;
	last_run_at: string | null;
	next_run_at: string | null;
	status: string;
	created_at: string;
	updated_at: string;
	/** Maximum wall-clock seconds the container may run. null = no timeout. */
	timeout_seconds: number | null;
	/** Environment variable keys; values are masked (write-only). */
	env: EnvVarEntry[];
	/** Denormalised last-run outcome: "success" | "failed" | null (never run). */
	last_run_status: string | null;
	/** Concurrency policy: "skip" (default) or "allow". */
	concurrency_policy: string;
	/** Memory limit in MiB. null = no limit. */
	memory_mb: number | null;
	/** Docker CPU shares (relative weight). null = default (1024). */
	cpu_shares: number | null;
	/** IANA timezone for schedule evaluation. null = UTC. */
	timezone: string | null;
	/** Number of automatic retries on non-zero exit. 0 = no retries. */
	max_retries: number;
	/** Seconds to wait between retries. */
	retry_delay_seconds: number;
	/** Volume mounts. null = none. */
	volumes: CronVolume[] | null;
}

export interface CronJobRun {
	id: string;
	cron_job_id: string;
	status: string;
	exit_code: number | null;
	output: string | null;
	started_at: string;
	finished_at: string | null;
	/** Retry attempt number: 1 = first attempt, 2 = first retry, etc. */
	attempt: number;
}

export async function listCronJobs(): Promise<CronJob[]> {
	return request<CronJob[]>('/cron');
}

export async function getCronJob(id: string): Promise<CronJob> {
	return request<CronJob>(`/cron/${id}`);
}

export async function createCronJob(data: {
	name: string;
	schedule: string;
	image: string;
	command?: string[];
	project_id?: string;
	timeout_seconds?: number | null;
	env?: Record<string, string>;
	/** "skip" (default) or "allow" */
	concurrency_policy?: string;
	/** Memory limit in MiB */
	memory_mb?: number | null;
	/** Docker CPU shares (relative weight, e.g. 512, 1024) */
	cpu_shares?: number | null;
	/** IANA timezone, e.g. "America/New_York". null = UTC. */
	timezone?: string | null;
	/** Number of automatic retries on non-zero exit. 0 = disabled. */
	max_retries?: number;
	/** Seconds to wait between retries. */
	retry_delay_seconds?: number;
	/** Volume mounts */
	volumes?: CronVolumeInput[] | null;
}): Promise<CronJob> {
	return request<CronJob>('/cron', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function updateCronJob(
	id: string,
	data: Partial<{
		/** Rename the job. Conflicts with existing names return 409. */
		name: string;
		/** Pass null to unlink from project, a project ID to link, omit to leave unchanged */
		project_id: string | null;
		schedule: string;
		image: string;
		/** Pass [] to clear to image default entrypoint */
		command: string[];
		enabled: boolean;
		/** Pass null to clear, a number to set, omit to leave unchanged */
		timeout_seconds: number | null;
		/** Replace all environment variables */
		env: Record<string, string>;
		/** Add or overwrite these variables (keeps the others) */
		env_set: Record<string, string>;
		/** Remove these variables */
		env_unset: string[];
		/** "skip" or "allow" */
		concurrency_policy: string;
		/** Memory limit in MiB. null = no limit. */
		memory_mb: number | null;
		/** Docker CPU shares. null = default (1024). */
		cpu_shares: number | null;
		/** IANA timezone. null = UTC. */
		timezone: string | null;
		/** Number of automatic retries. 0 = disabled. */
		max_retries: number;
		/** Seconds to wait between retries. */
		retry_delay_seconds: number;
		/** Volume mounts. null = remove all. */
		volumes: CronVolumeInput[] | null;
	}>
): Promise<CronJob> {
	return request<CronJob>(`/cron/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

export async function deleteCronJob(id: string): Promise<void> {
	await request(`/cron/${id}`, { method: 'DELETE' });
}

export async function triggerCronJob(id: string): Promise<void> {
	await request(`/cron/${id}/trigger`, { method: 'POST' });
}

export interface CronSchedulePreview {
	/** Next run times as RFC 3339 UTC timestamps (up to `count`). */
	next_runs: string[];
	/** Time zone the schedule is evaluated in ("UTC" when none is set). */
	timezone: string;
}

/** Validate a schedule and list its next runs, evaluated in `timezone` (IANA, empty = UTC). */
export async function previewCronSchedule(
	schedule: string,
	timezone?: string | null,
	count = 5
): Promise<CronSchedulePreview> {
	const params = new URLSearchParams({ schedule, count: String(count) });
	if (timezone) params.set('timezone', timezone);
	return request<CronSchedulePreview>(`/cron/schedule-preview?${params}`);
}

/**
 * Wait for a manually triggered cron run to finish, using the run stream (no
 * polling; the socket reconnects if the server restarts). Calls `onDone` once
 * a run that started at or after `since` completes, then closes the socket.
 * Returns a cancel function.
 */
export function watchCronRunCompletion(
	id: string,
	onDone: () => void,
	since: Date = new Date(Date.now() - 5000)
): () => void {
	let settled = false;
	const isNewFinishedRun = (run: { status?: string; started_at?: string } | null | undefined) =>
		!!run &&
		run.status !== 'running' &&
		!!run.started_at &&
		new Date(run.started_at).getTime() >= since.getTime();
	const socket = connectWithRetry(() => wsUrl(`${BASE}/cron/${id}/runs/stream`), {
		onMessage(event) {
			let frame: { type?: string; run?: { status?: string; started_at?: string } | null };
			try {
				frame = JSON.parse(event.data as string);
			} catch {
				return;
			}
			const done =
				frame.type === 'run' || (frame.type === 'snapshot' && isNewFinishedRun(frame.run));
			if (done && !settled) {
				settled = true;
				socket.close();
				onDone();
			}
		}
	});
	return () => {
		settled = true;
		socket.close();
	};
}

export async function listCronJobRuns(
	id: string,
	params?: { limit?: number; offset?: number }
): Promise<CronJobRun[]> {
	const qs = new URLSearchParams();
	if (params?.limit != null) qs.set('limit', String(params.limit));
	if (params?.offset != null) qs.set('offset', String(params.offset));
	const query = qs.toString() ? `?${qs.toString()}` : '';
	return request<CronJobRun[]>(`/cron/${id}/runs${query}`);
}

// ── Log streaming (WebSocket) ──────────────────────────────────────────────
//
// The backend sends JSON frames over a WebSocket at /api/apps/{id}/logs/stream.
//
// Frame types:
//   { type: "snapshot", deploy_logs: DeployLog[], container_logs: {ts: string|null, line: string}[] }
//     — sent once on connect with the most recent history
//   { type: "deploy_log", level: string, message: string, ts: string }
//     — incremental deploy log entry (polled every 1 s)
//   { type: "container", ts: string|null, line: string }
//     — incremental Docker stdout/stderr line

export interface LogLine {
	/** "deploy" | "container" */
	source: 'deploy' | 'container';
	level?: string; // deploy logs only ("info" | "warn" | "error")
	text: string;
	ts?: string;
	/** Present on system log stream frames; identifies the source app */
	appName?: string;
}

export interface LogStreamCallbacks {
	onLine: (line: LogLine) => void;
	/** Connected; `reconnected` is true after a drop. The server then sends a
	 *  fresh snapshot, so callers should clear what they show. */
	onOpen?: (reconnected: boolean) => void;
	/** Connection lost; the stream reconnects by itself in `retryInMs`. */
	onDisconnect?: (retryInMs: number) => void;
}

export type TerminalTargetKind = 'server' | 'app' | 'database';

export interface TerminalTarget {
	id: string;
	kind: TerminalTargetKind;
	label: string;
	shell_user: string | null;
	/** False for the host shell while it is switched off on the server. */
	enabled: boolean;
	/** Opening a session needs the password (and 2FA code). */
	requires_step_up: boolean;
}

/** Command that switches the host terminal on (only works on the server itself). */
export const HOST_TERMINAL_ENABLE_COMMAND = 'sudo tond settings host-terminal on';

export interface TerminalStepUp {
	password: string;
	/** TOTP or backup code, when 2FA is enabled. */
	code?: string;
}

export interface TerminalTicket {
	ticket: string;
	expires_in: number;
}

export interface TerminalSessionCallbacks {
	onReady?: () => void;
	onOutput: (data: string) => void;
	onExit?: (code: number | null) => void;
	onError?: (message: string) => void;
	onClose?: () => void;
}

export interface TerminalSessionHandle {
	send: (data: string) => void;
	resize: (cols: number, rows: number) => void;
	close: () => void;
}

/**
 * Converts a completed CronJobRun into one or more LogLine entries and
 * calls `callbacks.onLine` for each output line.
 */
function emitCronRun(
	run: {
		id: string;
		status: string;
		exit_code: number | null;
		output: string | null;
		started_at: string;
		finished_at: string | null;
	},
	callbacks: LogStreamCallbacks
): void {
	const ts = run.started_at;
	callbacks.onLine({ source: 'deploy', level: 'info', text: `── run ${run.id} started`, ts });
	if (run.output) {
		for (const line of run.output.split('\n')) {
			callbacks.onLine({ source: 'container', text: line, ts });
		}
	}
	const statusLevel = run.status === 'success' ? 'info' : 'error';
	const exitStr = run.exit_code !== null ? ` (exit ${run.exit_code})` : '';
	callbacks.onLine({
		source: 'deploy',
		level: statusLevel,
		text: `── run ${run.id} ${run.status}${exitStr}`,
		ts: run.finished_at ?? ts
	});
}

/**
 * Opens a WebSocket to `wsUrl` and invokes `callbacks.onLine` for every
 * incoming log line. Reconnects with backoff when the connection drops (e.g.
 * tond-server restarted). Returns a `close()` function — call it in $effect
 * cleanup.
 *
 * Pass a relative URL like `/api/apps/{id}/logs/stream`; it will be resolved
 * to `ws://` or `wss://` automatically based on the current page protocol.
 */
export function openLogStream(path: string, callbacks: LogStreamCallbacks): () => void {
	const socket = connectWithRetry(() => wsUrl(path), {
		onOpen: (reconnected) => callbacks.onOpen?.(reconnected),
		onDisconnect: (retryInMs) => callbacks.onDisconnect?.(retryInMs),
		onMessage: (ev) => handleLogFrame(ev, callbacks)
	});
	return () => socket.close();
}

function handleLogFrame(ev: MessageEvent, callbacks: LogStreamCallbacks) {
	let frame: any;
	try {
		frame = JSON.parse(ev.data as string);
	} catch {
		// Raw text line (shouldn't happen with this backend, but be defensive)
		callbacks.onLine({ source: 'container', text: ev.data as string });
		return;
	}

	if (frame.type === 'snapshot') {
		// App/database log snapshot: deploy_logs + container_logs arrays
		for (const dl of frame.deploy_logs ?? []) {
			callbacks.onLine({
				source: 'deploy',
				level: dl.level,
				text: dl.message,
				ts: dl.ts,
				appName: dl.app_name
			});
		}
		for (const line of frame.container_logs ?? []) {
			callbacks.onLine({ source: 'container', text: line.line, ts: line.ts ?? undefined });
		}
		// Cron run snapshot: single run object (may be null)
		if (frame.run) {
			emitCronRun(frame.run, callbacks);
		}
	} else if (frame.type === 'deploy') {
		callbacks.onLine({
			source: 'deploy',
			level: frame.level,
			text: frame.message,
			ts: frame.ts,
			appName: frame.app_name
		});
	} else if (frame.type === 'container') {
		callbacks.onLine({
			source: 'container',
			text: frame.line,
			ts: frame.ts ?? undefined,
			appName: frame.app_name
		});
	} else if (frame.type === 'run') {
		// Cron run completed frame
		emitCronRun(frame.run, callbacks);
	}
}

export async function listTerminalTargets(): Promise<TerminalTarget[]> {
	return request<TerminalTarget[]>('/terminal/targets');
}

/**
 * Get a single-use, 30-second ticket for one terminal WebSocket. The host shell
 * needs step-up credentials (password, plus a 2FA code when enabled).
 */
export async function createTerminalTicket(
	target: Pick<TerminalTarget, 'id' | 'kind'>,
	stepUp?: TerminalStepUp
): Promise<TerminalTicket> {
	return request<TerminalTicket>('/terminal/tickets', {
		method: 'POST',
		body: JSON.stringify({
			target_kind: target.kind,
			target_id: target.id,
			password: stepUp?.password,
			code: stepUp?.code || undefined
		})
	});
}

/** Open a terminal WebSocket with a ticket from `createTerminalTicket`. */
export function openTerminalSession(
	ticket: string,
	callbacks: TerminalSessionCallbacks
): TerminalSessionHandle {
	const protocol = window.location.protocol === 'https:' ? 'wss' : 'ws';
	const query = new URLSearchParams({ ticket });
	const ws = new WebSocket(
		`${protocol}://${window.location.host}${BASE}/terminal/ws?${query.toString()}`
	);

	ws.onmessage = (ev) => {
		let frame: any;
		try {
			frame = JSON.parse(ev.data as string);
		} catch {
			callbacks.onOutput(ev.data as string);
			return;
		}

		if (frame.type === 'ready') {
			callbacks.onReady?.();
		} else if (frame.type === 'output') {
			callbacks.onOutput(frame.data ?? '');
		} else if (frame.type === 'exit') {
			callbacks.onExit?.(frame.code ?? null);
		} else if (frame.type === 'error') {
			callbacks.onError?.(frame.message ?? 'Terminal session error');
		}
	};

	ws.onerror = () => callbacks.onError?.('Terminal connection failed');
	ws.onclose = () => callbacks.onClose?.();

	return {
		send: (data: string) => {
			if (ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'input', data }));
			}
		},
		resize: (cols: number, rows: number) => {
			if (ws.readyState === WebSocket.OPEN) {
				ws.send(JSON.stringify({ type: 'resize', cols, rows }));
			}
		},
		close: () => ws.close()
	};
}

// ── Notification Integrations ─────────────────────────────────────────────────

export type IntegrationKind =
	'telegram' | 'discord' | 'slack' | 'email' | 'ntfy' | 'pushover' | 'gotify' | 'webhook';

// Secret fields (bot_token, webhook_url, smtp_password, topic, token,
// user_key, app_token, signing_secret) are write-only: the server never
// returns them (see `Integration.saved_secrets`); an update that omits one, or
// sends "", keeps the saved value, and `null` clears an optional one.

export interface TelegramConfig {
	bot_token?: string;
	chat_id: string;
	thread_id?: string;
}

export interface DiscordConfig {
	webhook_url?: string;
}

export interface SlackConfig {
	webhook_url?: string;
}

export interface EmailConfig {
	from_name: string;
	from_addr: string;
	smtp_host: string;
	smtp_port: number;
	smtp_user: string;
	smtp_password?: string;
	/** "tls" | "starttls" | "none" */
	smtp_encryption: string;
	to_addrs: string[];
}

export interface NtfyConfig {
	server_url?: string;
	/** Secret: on a public server the topic name is the password */
	topic?: string;
	/** Access token for protected topics (null clears it) */
	token?: string | null;
}

export interface PushoverConfig {
	user_key?: string;
	app_token?: string;
}

export interface GotifyConfig {
	server_url?: string;
	app_token?: string;
}

export interface WebhookChannelConfig {
	webhook_url?: string;
	/** HMAC-SHA256 key for X-Tond-Signature-256 (null clears it) */
	signing_secret?: string | null;
}

export type IntegrationConfig =
	| TelegramConfig
	| DiscordConfig
	| SlackConfig
	| EmailConfig
	| NtfyConfig
	| PushoverConfig
	| GotifyConfig
	| WebhookChannelConfig;

export interface Integration {
	id: string;
	name: string;
	kind: IntegrationKind;
	/** Config without its secret fields */
	config: IntegrationConfig;
	/** Secret fields that have a saved value, e.g. ["bot_token"] */
	saved_secrets: string[];
	enabled: boolean;
	quiet_hours: ChannelQuietHours;
	/** Last successful delivery */
	last_sent_at: string | null;
	/** Last delivery error, until a delivery succeeds again */
	last_error: string | null;
	last_error_at: string | null;
	created_at: string;
	updated_at: string;
}

// API functions

export async function listIntegrations(): Promise<Integration[]> {
	return request<Integration[]>('/integrations');
}

export async function getIntegration(id: string): Promise<Integration> {
	return request<Integration>(`/integrations/${id}`);
}

export async function createIntegration(data: {
	name: string;
	kind: IntegrationKind;
	config: IntegrationConfig;
	quiet_hours?: ChannelQuietHours;
}): Promise<Integration> {
	return request<Integration>('/integrations', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function updateIntegration(
	id: string,
	data: {
		name?: string;
		config?: IntegrationConfig;
		enabled?: boolean;
		quiet_hours?: ChannelQuietHours;
	}
): Promise<Integration> {
	return request<Integration>(`/integrations/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

export async function deleteIntegration(id: string): Promise<void> {
	await request<{ deleted: string }>(`/integrations/${id}`, { method: 'DELETE' });
}

export async function testIntegration(id: string): Promise<{ message: string }> {
	return request<{ message: string }>(`/integrations/${id}/test`, { method: 'POST' });
}

// ── Storage Providers ─────────────────────────────────────────────────────────

export interface S3Config {
	endpoint: string | null;
	region: string;
	bucket: string;
	prefix: string | null;
	access_key_id: string;
	// secret_access_key not returned by API — send a new value to update it
}

export interface StorageProvider {
	id: string;
	name: string;
	kind: string; // currently always "s3"
	config: S3Config;
	created_at: string;
	updated_at: string;
}

export async function listStorageProviders(): Promise<StorageProvider[]> {
	return request<StorageProvider[]>('/storage-providers');
}

export async function createStorageProvider(data: {
	name: string;
	kind: string;
	config: S3Config & { secret_access_key: string };
}): Promise<StorageProvider> {
	return request<StorageProvider>('/storage-providers', {
		method: 'POST',
		body: JSON.stringify(data)
	});
}

export async function updateStorageProvider(
	id: string,
	data: { name?: string; config?: Partial<S3Config> & { secret_access_key?: string } }
): Promise<StorageProvider> {
	return request<StorageProvider>(`/storage-providers/${id}`, {
		method: 'PATCH',
		body: JSON.stringify(data)
	});
}

export async function deleteStorageProvider(id: string): Promise<void> {
	await request<{ deleted: string }>(`/storage-providers/${id}`, { method: 'DELETE' });
}

export async function testStorageProvider(id: string): Promise<{ ok: boolean; message: string }> {
	return request<{ ok: boolean; message: string }>(`/storage-providers/${id}/test`, {
		method: 'POST'
	});
}

// ── Snapshots ─────────────────────────────────────────────────────────────────

export interface Snapshot {
	id: string;
	name: string;
	status: string; // "pending" | "running" | "completed" | "failed"
	trigger: string; // "scheduled" | "manual" | "pre_deploy" | "pre_restore"
	scope: string; // "full" | "app:name" | "database:name"
	trigger_app: string | null;
	deployment_id: string | null;
	parent_snapshot_id: string | null;
	created_at: string;
	completed_at: string | null;
	delta_bytes: number | null;
	total_bytes: number | null;
	local_path: string | null;
	remote_key: string | null;
	storage_provider_id: string | null;
	/** SHA-256 of the encrypted archive in remote storage */
	checksum_sha256: string | null;
	error_message: string | null;
	notes: string | null;
	/** null (local only) | "uploading" | "uploaded" | "failed" */
	remote_status: 'uploading' | 'uploaded' | 'failed' | null;
	/** Why the last remote upload failed */
	remote_error: string | null;
	/** Size of the encrypted archive in remote storage */
	archive_bytes: number | null;
}

export interface BackupConfig {
	enabled: boolean;
	full_schedule_cron: string;
	full_retention_count: number;
	pre_deploy_enabled: boolean;
	pre_deploy_retention_per_app: number;
	keep_local: boolean;
	storage_provider_id: string | null;
}

export async function createSnapshot(name?: string): Promise<Snapshot> {
	return request<Snapshot>('/snapshots', {
		method: 'POST',
		body: JSON.stringify({ name: name ?? null })
	});
}

export async function listSnapshots(params?: {
	scope?: string;
	trigger?: string;
	app?: string;
}): Promise<Snapshot[]> {
	const qs = new URLSearchParams();
	if (params?.scope) qs.set('scope', params.scope);
	if (params?.trigger) qs.set('trigger', params.trigger);
	if (params?.app) qs.set('app', params.app);
	const query = qs.toString();
	return request<Snapshot[]>(`/snapshots${query ? `?${query}` : ''}`);
}

export async function deleteSnapshot(id: string): Promise<void> {
	await request<{ deleted: string }>(`/snapshots/${id}`, { method: 'DELETE' });
}

export async function restoreSnapshot(
	id: string
): Promise<{ pre_restore_snapshot_id: string; message: string }> {
	return request<{ pre_restore_snapshot_id: string; message: string }>(`/snapshots/${id}/restore`, {
		method: 'POST'
	});
}

export async function rollbackAppToSnapshot(
	snapshotId: string,
	appName: string
): Promise<{ message: string }> {
	return request<{ message: string }>(`/snapshots/${snapshotId}/restore-app`, {
		method: 'POST',
		body: JSON.stringify({ app_name: appName })
	});
}

export interface RestoreDatabaseResult {
	message: string;
	/** Backup of the data as it was just before the restore */
	pre_restore_snapshot_id: string;
}

/**
 * Restore one database from a snapshot that holds it. The server backs up the
 * current data first and returns that backup's id.
 */
export async function restoreDatabase(
	snapshotId: string,
	dbName: string
): Promise<RestoreDatabaseResult> {
	return request<RestoreDatabaseResult>(`/snapshots/${snapshotId}/restore-database`, {
		method: 'POST',
		body: JSON.stringify({ db_name: dbName })
	});
}

export async function getBackupConfig(): Promise<BackupConfig> {
	return request<BackupConfig>('/snapshots/config');
}

export async function updateBackupConfig(config: Partial<BackupConfig>): Promise<BackupConfig> {
	return request<BackupConfig>('/snapshots/config', {
		method: 'PUT',
		body: JSON.stringify(config)
	});
}
// Download: window.open(`/api/snapshots/${id}/download`) — no fetch wrapper needed
