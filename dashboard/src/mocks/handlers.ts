/**
 * MSW request handlers for all /api/* endpoints.
 * Every handler returns the ApiResponse<T> envelope the real server uses:
 *   { success: true, data: T, error: null }
 */

import { http, HttpResponse, ws, delay } from 'msw';

// WebSocket links must be declared at module scope so MSW can track clients
// across the service worker boundary via a stable BroadcastChannel. Use paths
// relative to the page origin: a wildcard host ("ws://*/…") never matches.
const appLogStream = ws.link('/api/apps/:id/logs/stream');
const systemLogStream = ws.link('/api/system/logs/stream');
const allDatabaseLogStream = ws.link('/api/databases/logs/stream');
const databaseLogStream = ws.link('/api/databases/:id/logs/stream');
const cronRunStream = ws.link('/api/cron/:id/runs/stream');
const terminalStream = ws.link('/api/terminal/ws');
const deploymentStream = ws.link('/api/apps/:id/deployments/:deploymentId/stream');

import {
	MOCK_USER,
	MOCK_USERS,
	MOCK_APPS,
	MOCK_PROJECTS,
	MOCK_TEMPLATES,
	MOCK_TEMPLATE_MEMORY,
	MOCK_SETTINGS,
	MOCK_RETENTION,
	MOCK_AUDIT_SHIPPING,
	makeMockAppEvents,
	MOCK_ONBOARDING_STATUS,
	MOCK_DEPLOYMENTS,
	MOCK_DEPLOYMENT_LOGS,
	mockBuildLog,
	MOCK_GENERATED_DOCKERFILES,
	MOCK_DEPLOY_KEYS,
	MOCK_PREVIEWS,
	MOCK_PROJECT_PREVIEWS,
	MOCK_AUDIT_LOG,
	MOCK_TOKEN_CATALOG,
	MOCK_TOKENS,
	MOCK_DATABASES,
	MOCK_CRON_JOBS,
	MOCK_CRON_RUNS,
	MOCK_REGISTRY_CREDENTIALS,
	MOCK_GITHUB_APPS,
	MOCK_GITHUB_REPOS,
	MOCK_GITHUB_BRANCHES,
	MOCK_INTEGRATIONS,
	MOCK_STORAGE_PROVIDERS,
	MOCK_SNAPSHOTS,
	MOCK_BACKUP_CONFIG,
	MOCK_APP_ENV_VARS,
	MOCK_PREVIEW_ENV_VARS,
	MOCK_CERTIFICATES,
	MOCK_DATABASE_LINKS,
	makeMockSystemMetrics,
	makeMockSystemSelf,
	makeMockMetrics,
	makeMockRequestMetrics,
	makeMockTrafficOverview,
	makeMockAccessLog,
	MOCK_DATABASE_SECRETS
} from './fixtures';
import type {
	UpdateAuditShipping,
	ActiveAlert,
	AlertEpisode,
	AlertRule,
	AlertRuleInput,
	AlertRuleUpdate,
	AlertSettings,
	App,
	BackupConfig,
	CronJob,
	CronJobRun,
	Deployment,
	DeploymentLogLine,
	DeployPhase,
	PhaseRecord,
	GitHubApp,
	GithubPR,
	Integration,
	ManagedDatabase,
	Template,
	TemplateDeployRequest,
	Volume,
	VolumeInput,
	CronVolume,
	CronVolumeInput,
	PreviewDeployment,
	Project,
	ProjectPreview,
	RegistryCredential,
	Snapshot,
	StorageProvider,
	EnvVarEntry,
	DatabaseCredentials
} from '$lib/api';
import {
	MOCK_ACTIVE_ALERTS,
	MOCK_ALERT_DELIVERIES,
	MOCK_ALERT_HISTORY,
	MOCK_ALERT_KINDS,
	MOCK_ALERT_RULES,
	MOCK_ALERT_SETTINGS
} from './fixtures';
import { observeHandlers } from './observe';

// Simulated network latency (ms). Set to 0 for instant responses.
const LATENCY = 200;

function ok<T>(data: T) {
	return HttpResponse.json({ success: true, data, error: null });
}

/** 24 random letters and digits, like the server's generated passwords. */
function mockGeneratedSecret(): string {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
	const bytes = crypto.getRandomValues(new Uint8Array(24));
	return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join('');
}

function err(message: string, status = 400, code?: string) {
	return HttpResponse.json(
		{ success: false, data: null, error: message, ...(code ? { code } : {}) },
		{ status }
	);
}

/** The mock password policy: 12–256 characters, no common passwords. */
function mockPasswordProblem(password: string): string | null {
	const n = [...password].length;
	if (n < 12)
		return "Use at least 12 characters. A few random words make a strong password that's easy to remember.";
	if (n > 256) return 'Use at most 256 characters.';
	const core = password.toLowerCase().replace(/^[\d\W]+|[\d\W]+$/g, '');
	if (['password', 'qwerty', 'iloveyou', 'welcome', 'admin', 'letmein'].includes(core)) {
		return "This password is too common: it's on lists of breached passwords that attackers try first. Pick something unique, like a few random words.";
	}
	return null;
}

/** Valid set-password link token in the mock (any other token is refused). */
const MOCK_PASSWORD_LINK = 'mock-link';

// ── Mock helpers mirroring server-side confinement rules ─────────────────────

const MOCK_VOLUMES_DIR = '/opt/tond/data/volumes';

/** "/data/cache" → "data_cache" (same default as the server). */
function mockVolumeName(containerPath: string): string {
	return (
		containerPath
			.replace(/^\/+|\/+$/g, '')
			.replace(/[^A-Za-z0-9._-]/g, '_')
			.replace(/^\.+/, '') || 'data'
	);
}

/** Validate a volume request like the API does; returns an error message or null. */
function mockVolumeError(
	input: { container_path?: string; name?: string; host_path?: string },
	baseDir: string
): string | null {
	const path = input.container_path ?? '';
	if (
		!path.startsWith('/') ||
		path === '/' ||
		/[:,]/.test(path) ||
		/(^|\/)\.\.?(\/|$)/.test(path)
	) {
		return `Container path '${path}' must be an absolute path inside the container, e.g. /data`;
	}
	if (input.name && !/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,63}$/.test(input.name)) {
		return `Volume name '${input.name}' may only contain letters, digits, '.', '_' and '-' (no slashes)`;
	}
	if (
		input.host_path &&
		!new RegExp(`^${baseDir}/[A-Za-z0-9_-][A-Za-z0-9._-]*$`).test(input.host_path)
	) {
		return `Host path '${input.host_path}' is not allowed. Volumes are stored inside ${baseDir}; give the volume a name (e.g. "data") instead of a host path.`;
	}
	return null;
}

function mockAppVolume(appName: string, input: VolumeInput & { host_path?: string }): Volume {
	const base = `${MOCK_VOLUMES_DIR}/${appName}`;
	const name =
		input.name ||
		(input.host_path ? input.host_path.slice(base.length + 1) : '') ||
		mockVolumeName(input.container_path);
	return { name, host_path: `${base}/${name}`, container_path: input.container_path };
}

function mockCronVolumes(
	jobId: string,
	inputs: CronVolumeInput[] | null | undefined
): CronVolume[] | null {
	if (!inputs || inputs.length === 0) return null;
	return inputs.map((v) => {
		const name = v.name || mockVolumeName(v.container_path);
		const base = v.app ? `${MOCK_VOLUMES_DIR}/${v.app}` : `${MOCK_VOLUMES_DIR}/_cron/${jobId}`;
		return {
			app: v.app ?? null,
			name,
			host_path: `${base}/${name}`,
			container_path: v.container_path
		};
	});
}

/** A few of the server's compose rules, enough to demo the UI. */
function mockComposeProblems(content: string): string[] {
	const problems: string[] = [];
	if (/^\s*privileged:\s*true/m.test(content)) {
		problems.push(
			"service: 'privileged' is not allowed — privileged containers have full root access to the host"
		);
	}
	for (const field of ['network_mode', 'pid', 'ipc']) {
		if (new RegExp(`^\\s*${field}:\\s*["']?host`, 'm').test(content)) {
			problems.push(`service: '${field}: host' is not allowed`);
		}
	}
	if (/^\s*cap_add:/m.test(content)) {
		problems.push("service: 'cap_add' beyond Docker's default set is not allowed");
	}
	for (const m of content.matchAll(/^\s*-\s*["']?(\/[^:"'\s]*):/gm)) {
		problems.push(
			`service: bind mount '${m[1]}' is outside this app's volume directory — use a relative path such as ./data or a named volume`
		);
	}
	return problems;
}

/** Public connection string (only when public access is on), plaintext. */
function mockPublicUrl(db: ManagedDatabase, password: string | null): string | null {
	if (!db.public_access || !db.host_port) return null;
	const isRedisLike = db.db_type === 'redis' || db.db_type === 'keydb';
	return isRedisLike
		? `redis://${db.db_user}:${password}@tond.example.com:${db.host_port}`
		: `${db.db_type}://${db.db_user}:${password}@tond.example.com:${db.host_port}/${db.db_name}`;
}

// Mirrors tond-server's domains.rs: previews need a preview domain on a different
// registrable domain from the dashboard. The mock approximates the Public Suffix
// List check by comparing the last two labels.
const PREVIEW_DOMAIN_REQUIRED =
	'Invalid input: Previews need a preview domain. Set one in Settings: a separate domain you own (e.g. example-previews.com, not a subdomain of the dashboard domain) with a wildcard DNS record *.<preview-domain> pointing at this server. Previews run pull-request code, so they must not share a site with the dashboard.';

function mockSite(domain: string): string {
	return domain.split('.').slice(-2).join('.');
}

function mockPreviewDomainError(preview: string, dashboard: string): string | null {
	if (!preview) return null;
	if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(preview)) {
		return `'${preview}' is not a valid domain name (expected something like example-previews.com)`;
	}
	if (!dashboard) return null;
	const nested =
		preview === dashboard || preview.endsWith(`.${dashboard}`) || dashboard.endsWith(`.${preview}`);
	if (nested || mockSite(preview) === mockSite(dashboard)) {
		return `The preview domain must be a different registrable domain from the dashboard (${dashboard}). Previews run pull-request code; use a separate domain you own.`;
	}
	return null;
}

function normalizeDomain(value: string): string {
	return value.trim().toLowerCase().replace(/\.$/, '');
}

/** Two hostnames on the same site (approximated by their last two labels). */
function mockSameSite(a: string, b: string): boolean {
	if (!a || !b) return false;
	return a === b || a.endsWith(`.${b}`) || b.endsWith(`.${a}`) || mockSite(a) === mockSite(b);
}

// Mirrors domains.rs validate_apps_domain.
function mockAppsDomainError(apps: string, dashboard: string, preview: string): string | null {
	if (!apps) return null;
	if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(apps)) {
		return `'${apps}' is not a valid domain name (expected something like example-apps.com)`;
	}
	if (mockSameSite(apps, dashboard)) {
		return `The apps domain must be a different site from the dashboard (${dashboard}): every app would be same-site with the dashboard. Use a separate domain, such as example-apps.com.`;
	}
	if (mockSameSite(apps, preview)) {
		return `The apps domain must be a different site from the preview domain (${preview}). Previews run pull-request code and must not share a site with your production apps.`;
	}
	return null;
}

function mockDomainWarning(domain: string | null): string | null {
	const dashboard = MOCK_SETTINGS.dashboard_domain;
	if (!domain || !mockSameSite(domain, dashboard)) return null;
	return `${domain} is on the same site as your dashboard (${dashboard}). The app can send same-site requests to the dashboard and set cookies for it; use a domain on a different site for code you don't fully trust.`;
}

// ── Database links (networks.rs) ──
let databaseLinks = structuredClone(MOCK_DATABASE_LINKS);

const DB_PORTS: Record<string, number> = {
	postgres: 5432,
	mysql: 3306,
	mariadb: 3306,
	redis: 6379,
	keydb: 6379,
	mongodb: 27017
};

function mockDatabaseAccess(db: ManagedDatabase) {
	const ref = (a: App) => ({
		id: a.id,
		name: a.name,
		status: a.status,
		project_id: a.project_id
	});
	const project = db.project_id ? projects.find((p) => p.id === db.project_id) : undefined;
	return {
		database_id: db.id,
		project: project ? { id: project.id, name: project.name } : null,
		project_apps: db.project_id ? apps.filter((a) => a.project_id === db.project_id).map(ref) : [],
		linked_apps: databaseLinks
			.filter((l) => l.database_id === db.id)
			.map((l) => apps.find((a) => a.id === l.app_id))
			.filter((a): a is App => !!a)
			.map(ref),
		internal_host: db.name,
		internal_port: DB_PORTS[db.db_type] ?? 0
	};
}

const certificates = MOCK_CERTIFICATES.map((c) => ({ ...c }));

/** Mirror the server: saving a domain queues a pending cert and resets retry state. */
function queueMockCertificate(domain: string) {
	if (!domain.trim()) return;
	const now = new Date().toISOString();
	const existing = certificates.find((c) => c.domain === domain);
	if (existing) {
		Object.assign(existing, {
			status: 'pending',
			last_error: null,
			next_retry_at: null,
			failure_count: 0
		});
		return;
	}
	certificates.push({
		id: `cert-${Date.now()}`,
		domain,
		cert_type: 'acme',
		status: 'pending',
		issued_at: now,
		expires_at: now,
		last_error: null,
		last_attempt_at: null,
		next_retry_at: null,
		failure_count: 0,
		issuer: null,
		issuer_kind: 'unknown',
		last_renewed_at: null,
		serves: { kind: 'unused' }
	});
}

// In-memory mutable state (mutations survive page navigation within the session)
// Starts as true so that `pnpm dev:setup` begins signed out with setup open.
// Like the real server, /api/auth/status doesn't reveal it: open the setup link
// /setup?token=mock-setup-token (MOCK_SETUP_TOKEN) to reach the form.
// Flips to false once POST /api/auth/setup succeeds.
let needsSetup = true;
export const MOCK_SETUP_TOKEN = 'mock-setup-token';

// The host terminal is off by default, as on a real server (where only
// `sudo tond settings host-terminal on` can enable it). To preview the step-up
// flow in the mock, run `localStorage.setItem('tond-mock-host-terminal', 'on')`.
function mockHostTerminalEnabled(): boolean {
	try {
		return globalThis.localStorage?.getItem('tond-mock-host-terminal') === 'on';
	} catch {
		return false;
	}
}
/** Tiny cron evaluator for the mock schedule preview (UTC, minute resolution). */
function mockNextRuns(schedule: string, count: number): string[] | null {
	const fields = schedule.trim().split(/\s+/);
	if (fields.length !== 5) return null;
	const ranges: [number, number][] = [
		[0, 59],
		[0, 23],
		[1, 31],
		[1, 12],
		[0, 7]
	];
	const sets: Set<number>[] = [];
	for (let i = 0; i < 5; i++) {
		const [min, max] = ranges[i];
		const set = new Set<number>();
		for (const part of fields[i].split(',')) {
			const [range, stepStr] = part.split('/');
			const step = stepStr ? Number(stepStr) : 1;
			if (!Number.isInteger(step) || step < 1) return null;
			const [a, rawB] = range === '*' ? [min, max] : range.split('-').map(Number);
			const b = rawB === undefined ? (stepStr ? max : a) : rawB;
			if (!Number.isInteger(a) || !Number.isInteger(b) || a < min || b > max || a > b) return null;
			for (let v = a; v <= b; v += step) set.add(i === 4 ? v % 7 : v);
		}
		sets.push(set);
	}
	const runs: string[] = [];
	const t = new Date();
	t.setUTCSeconds(0, 0);
	for (let n = 0; n < 60 * 24 * 400 && runs.length < count; n++) {
		t.setUTCMinutes(t.getUTCMinutes() + 1);
		if (
			sets[0].has(t.getUTCMinutes()) &&
			sets[1].has(t.getUTCHours()) &&
			sets[2].has(t.getUTCDate()) &&
			sets[3].has(t.getUTCMonth() + 1) &&
			sets[4].has(t.getUTCDay())
		) {
			runs.push(t.toISOString());
		}
	}
	return runs;
}

const MOCK_STEP_UP_PASSWORD = 'ValidPass123!';
// ticket → target, consumed by the terminal WebSocket mock
const terminalTickets = new Map<string, { kind: string; id: string }>();
let apps: App[] = structuredClone(MOCK_APPS);
let projects: Project[] = structuredClone(MOCK_PROJECTS);
const templates: Template[] = structuredClone(MOCK_TEMPLATES);
let alertRules: AlertRule[] = structuredClone(MOCK_ALERT_RULES);
let activeAlerts: ActiveAlert[] = structuredClone(MOCK_ACTIVE_ALERTS);
const alertHistory: AlertEpisode[] = structuredClone(MOCK_ALERT_HISTORY);
let alertSettings: AlertSettings = structuredClone(MOCK_ALERT_SETTINGS);
const deployments: Record<string, Deployment[]> = structuredClone(MOCK_DEPLOYMENTS);
let previews: Record<string, PreviewDeployment[]> = structuredClone(MOCK_PREVIEWS);
/** Opt-in access logs per app id (the first running app starts with one). */
let accessLogs: Record<string, { enabled: boolean; include_query: boolean; lines: string[] }> = {
	[MOCK_APPS.find((a) => a.status === 'running')?.id ?? '']: {
		enabled: true,
		include_query: false,
		lines: makeMockAccessLog()
	}
};
let projectPreviews: Record<string, ProjectPreview[]> = structuredClone(MOCK_PROJECT_PREVIEWS);
let databases: ManagedDatabase[] = structuredClone(MOCK_DATABASES);
let cronJobs: CronJob[] = structuredClone(MOCK_CRON_JOBS);
const cronRuns: Record<string, CronJobRun[]> = structuredClone(MOCK_CRON_RUNS);
let registryCredentials: RegistryCredential[] = structuredClone(MOCK_REGISTRY_CREDENTIALS);
let githubApps: GitHubApp[] = structuredClone(MOCK_GITHUB_APPS);

// Per-app webhook secrets (the real server generates one at app creation).
const webhookSecrets: Record<string, string> = {};

// Stored secrets live beside the rows, like the server's encrypted columns:
// responses only ever carry masked values or `has_*` flags.
const ENV_MASK = '••••••••';

/** Mirrors tond_common::mask_url_password. */
function maskUrlPassword(url: string): string {
	return url.replace(/(:\/\/[^:/@\s]+):[^/\s]+@/g, `$1:${ENV_MASK}@`);
}

const INTEGRATION_SECRET_FIELDS = [
	'bot_token',
	'webhook_url',
	'smtp_password',
	'topic',
	'token',
	'user_key',
	'app_token',
	'signing_secret'
] as const;
const integrationSecrets: Record<string, Record<string, string>> = {};

/** Split a submitted integration config into its plain part and saved secrets. */
function storeIntegrationConfig(
	id: string,
	config: Record<string, unknown>
): { config: Integration['config']; saved_secrets: string[] } {
	const saved = (integrationSecrets[id] ??= {});
	const plain: Record<string, unknown> = { ...config };
	for (const field of INTEGRATION_SECRET_FIELDS) {
		const value = plain[field];
		delete plain[field];
		if (typeof value === 'string' && value.trim()) saved[field] = value.trim();
		else if (value === null) delete saved[field];
	}
	return {
		config: plain as unknown as Integration['config'],
		saved_secrets: INTEGRATION_SECRET_FIELDS.filter((f) => saved[f])
	};
}

const cronEnv: Record<string, Record<string, string>> = Object.fromEntries(
	cronJobs.map((job) => [job.id, Object.fromEntries(job.env.map((e) => [e.key, 'mock-value']))])
);
const projectEnv: Record<string, Record<string, string>> = {};

// Managed database secrets, stored beside the rows (see MOCK_DATABASE_SECRETS).
const dbSecrets: Record<string, { db_password: string | null; root_password: string | null }> =
	structuredClone(MOCK_DATABASE_SECRETS);
/** Plaintext connection-string overrides (rows carry the masked form). */
const dbCustomUrls: Record<string, { external: string | null; internal: string | null }> = {};

/** Plaintext connection strings, as the credentials reveal returns them. */
function dbCredentials(db: ManagedDatabase): DatabaseCredentials {
	const password = dbSecrets[db.id]?.db_password ?? null;
	const isRedisLike = db.db_type === 'redis' || db.db_type === 'keydb';
	const port = db.db_type === 'mysql' || db.db_type === 'mariadb' ? 3306 : 5432;
	const auth = db.db_user && password ? `${db.db_user}:${password}@` : '';
	const computedExternal = db.host_port
		? isRedisLike
			? `redis://${auth}localhost:${db.host_port}`
			: `${db.db_type}://${auth}localhost:${db.host_port}/${db.db_name}`
		: null;
	// Apps with access (project members, linked apps) reach it by its name.
	const computedInternal = isRedisLike
		? `redis://${auth}${db.name}:6379`
		: `${db.db_type}://${auth}${db.name}:${port}/${db.db_name}`;
	const custom = dbCustomUrls[db.id];
	return {
		db_user: db.db_user,
		db_password: password,
		root_password: dbSecrets[db.id]?.root_password ?? null,
		connection_string: custom?.external ?? computedExternal,
		public_connection_string: mockPublicUrl(db, password),
		internal_connection_string: custom?.internal ?? computedInternal
	};
}

/** A database detail response: connection strings with the password masked. */
function dbDetail(db: ManagedDatabase) {
	const creds = dbCredentials(db);
	const mask = (url: string | null) => (url ? maskUrlPassword(url) : null);
	return {
		...db,
		connection_string: mask(creds.connection_string),
		public_connection_string: mask(creds.public_connection_string),
		internal_connection_string: mask(creds.internal_connection_string)
	};
}

function mockDatabaseSnapshot(dbName: string, trigger: 'manual' | 'pre_restore'): Snapshot {
	const now = new Date().toISOString();
	const stamp = new Date().toISOString().slice(0, 16).replace('T', ' ');
	return {
		id: `snap_${trigger}_${Date.now()}`,
		name: trigger === 'manual' ? `${dbName} ${stamp}` : `pre-restore ${dbName} ${stamp}`,
		status: 'completed',
		trigger,
		scope: `database:${dbName}`,
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: now,
		completed_at: now,
		delta_bytes: 3_145_728,
		total_bytes: 3_145_728,
		local_path: `/opt/tond/data/backups/snapshots/snap_${Date.now()}`,
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null,
		remote_status: backupConfig.storage_provider_id ? 'uploading' : null,
		remote_error: null,
		archive_bytes: null
	};
}

function randomHex(bytes: number): string {
	const buf = new Uint8Array(bytes);
	crypto.getRandomValues(buf);
	return Array.from(buf, (b) => b.toString(16).padStart(2, '0')).join('');
}

function webhookConfig(app: App) {
	webhookSecrets[app.id] ??= randomHex(32);
	const paths = {
		github: `/api/webhooks/github/${app.id}`,
		gitlab: `/api/webhooks/gitlab/${app.id}`,
		gitea: `/api/webhooks/gitea/${app.id}`,
		generic: `/api/webhooks/generic/${app.id}`
	};
	const domain = MOCK_SETTINGS.dashboard_domain.trim();
	const urls = domain
		? {
				github: `https://${domain}${paths.github}`,
				gitlab: `https://${domain}${paths.gitlab}`,
				gitea: `https://${domain}${paths.gitea}`,
				generic: `https://${domain}${paths.generic}`
			}
		: null;
	return {
		app_id: app.id,
		paths,
		urls,
		has_secret: true,
		auto_deploy: app.auto_deploy,
		watch_branch: app.watch_branch
	};
}
let users = structuredClone(MOCK_USERS);
let apiTokens = structuredClone(MOCK_TOKENS);
const auditLog = structuredClone(MOCK_AUDIT_LOG);
let integrations: Integration[] = structuredClone(MOCK_INTEGRATIONS);
let storageProviders: StorageProvider[] = structuredClone(MOCK_STORAGE_PROVIDERS);
let snapshots: Snapshot[] = structuredClone(MOCK_SNAPSHOTS);
let backupConfig: BackupConfig = structuredClone(MOCK_BACKUP_CONFIG);
// app_id → env vars / preview overrides (plaintext; only masked values leave the mock)
const appEnvVars: Record<string, Record<string, string>> = structuredClone(MOCK_APP_ENV_VARS);
const previewEnvVars: Record<string, Record<string, string>> = structuredClone(
	MOCK_PREVIEW_ENV_VARS
);

function envStore(appId: string) {
	return (appEnvVars[appId] ??= {});
}

function previewStore(appId: string) {
	return (previewEnvVars[appId] ??= {});
}

function maskedEntries(vars: Record<string, string>): EnvVarEntry[] {
	return Object.keys(vars)
		.sort()
		.map((key) => ({ key, masked: vars[key] ? '••••••••' : '' }));
}

function appEnvResponse(app: App) {
	return {
		vars: maskedEntries(envStore(app.id)),
		preview: {
			enabled: app.preview_enabled,
			shared_keys: app.preview_shared_keys,
			overrides: maskedEntries(previewStore(app.id))
		}
	};
}

function validateEnvPatch(body: { set?: Record<string, string>; unset?: string[] }) {
	const set = Object.keys(body.set ?? {});
	const unset = body.unset ?? [];
	if (set.length === 0 && unset.length === 0) {
		return 'Invalid input: nothing to change: send at least one variable to set or unset';
	}
	const bad = [...set, ...unset].find((key) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key));
	if (bad) return `Invalid input: '${bad}' is not a valid variable name`;
	return null;
}

function applyEnvPatch(
	current: Record<string, string>,
	body: { set?: Record<string, string>; unset?: string[] }
) {
	const next = { ...current, ...(body.set ?? {}) };
	for (const key of body.unset ?? []) delete next[key];
	return next;
}

// ── Mock scenarios ──────────────────────────────────────────────────────────
// Reproduce failure states in the browser. Set one in the devtools console,
// then reload:  localStorage.setItem('tond:mock-scenario', 'session-expired')
//
//   server-down      every /api request gets a proxy-style 502 HTML page
//   session-expired  the next data request gets a 401 (one-shot), then normal
//   rate-limited     POST /api/auth/login gets 429 with Retry-After: 30
//   logged-out       start signed out (run with VITE_DEV_AUTH_BYPASS=false)
//   mfa              like logged-out, and sign-in asks for a 2FA code (123456)
//   2fa-enabled      the current user starts with 2FA enabled
//
// Clear with localStorage.removeItem('tond:mock-scenario').
const SCENARIO_KEY = 'tond:mock-scenario';

function mockScenario(): string | null {
	try {
		return globalThis.localStorage?.getItem(SCENARIO_KEY) ?? null;
	} catch {
		return null;
	}
}

function clearMockScenario() {
	try {
		globalThis.localStorage?.removeItem(SCENARIO_KEY);
	} catch {
		// ignore
	}
}

let mockSignedIn = false;
let mockTotpEnabled = mockScenario() === '2fa-enabled';

function currentMockUser() {
	return { ...MOCK_USER, totp_enabled: mockTotpEnabled };
}

const CREDENTIAL_PATHS = ['/api/auth/'];

// ── Mock deploy engine ────────────────────────────────────────────────────────
//
// POST /api/apps/:id/deploy (and rollback) start a scripted deployment that
// walks through the phases and writes log lines over ~15 s, so the live deploy
// view can be developed without a server. Apps named "worker-queue" fail the
// health step (nothing listening on the port); everything else succeeds.

const deploymentLogs: Record<string, DeploymentLogLine[]> = structuredClone(MOCK_DEPLOYMENT_LOGS);
type DeployFrame = { type: 'log' | 'deployment'; [key: string]: unknown };
const deployListeners = new Map<string, Set<(frame: DeployFrame) => void>>();

function emitDeploy(deploymentId: string, frame: DeployFrame) {
	for (const listener of deployListeners.get(deploymentId) ?? []) listener(frame);
}

function mockPhasePlan(plan: string, healthPath: string | null): PhaseRecord[] {
	const health = healthPath ? 'Health check' : 'Wait for the app to listen';
	const table: Record<string, (string | null)[]> = {
		git: ['Clone repository', 'Build image', 'Start container', health, 'Switch traffic'],
		image: ['Pull image', null, 'Start container', health, 'Switch traffic'],
		tarball: [null, 'Build uploaded source', 'Start container', health, 'Switch traffic'],
		dockerfile: [null, 'Build image', 'Start container', health, 'Switch traffic'],
		compose: [null, 'docker compose up', 'Find the service port', null, 'Switch traffic'],
		rollback: [null, null, 'Start container', health, 'Switch traffic']
	};
	const labels = table[plan] ?? table.rollback;
	const phases: DeployPhase[] = ['clone', 'build', 'start', 'health', 'release'];
	return phases.map((phase, i) =>
		labels[i] === null
			? { phase, status: 'skipped' as const }
			: { phase, status: 'pending' as const, label: labels[i] as string }
	);
}

function updateDeployment(appId: string, id: string, change: (d: Deployment) => Deployment) {
	const list = deployments[appId] ?? [];
	const idx = list.findIndex((d) => d.id === id);
	if (idx === -1) return;
	list[idx] = change(list[idx]);
	emitDeploy(id, { type: 'deployment', deployment: list[idx] });
}

function startMockDeploy(
	app: App,
	opts: { trigger_source: string; trigger_detail: string | null; rollbackOf?: Deployment }
): Deployment {
	const id = `dep_${Date.now().toString(36)}`;
	const now = new Date().toISOString();
	const plan = opts.rollbackOf ? 'rollback' : app.source_type;
	const deployment: Deployment = {
		id,
		app_id: app.id,
		image_tag: opts.rollbackOf?.image_tag ?? null,
		status: 'running',
		trigger_source: opts.trigger_source,
		trigger_detail: opts.trigger_detail,
		rollback_of: opts.rollbackOf?.id ?? null,
		commit_sha: opts.rollbackOf?.commit_sha ?? null,
		commit_message: opts.rollbackOf?.commit_message ?? null,
		commit_author: opts.rollbackOf?.commit_author ?? null,
		has_generated_dockerfile: false,
		started_at: now,
		finished_at: null,
		created_at: now,
		duration_secs: null,
		phases: mockPhasePlan(plan, app.health_check_path).map((p) =>
			p.phase === 'health' && p.status !== 'skipped' && app.readiness === 'none'
				? { ...p, label: `Stay up for ${app.ready_after_secs ?? 10}s` }
				: p.phase === 'health' && p.status !== 'skipped' && app.readiness === 'tcp'
					? { ...p, label: 'Wait for the port to accept connections' }
					: p.phase === 'release' && app.readiness === 'none'
						? { ...p, label: 'Replace the old container' }
						: p
		),
		failure: null,
		image_pruned: false,
		is_current: false,
		rollback_available: false,
		rollback_unavailable_reason: null
	};
	deployments[app.id] = [deployment, ...(deployments[app.id] ?? [])];
	deploymentLogs[id] = [];
	const appIdx = apps.findIndex((a) => a.id === app.id);
	if (appIdx !== -1) apps[appIdx] = { ...apps[appIdx], status: 'building' };

	const fail = app.name === 'worker-queue';
	const tag = `tond-${app.name}:${id.slice(4, 16).padEnd(12, '0')}`;
	const log = (level: string, message: string) => {
		const line = { level, message, ts: new Date().toISOString() };
		deploymentLogs[id].push(line);
		emitDeploy(id, { type: 'log', ...line });
	};
	const phase = (name: DeployPhase) =>
		updateDeployment(app.id, id, (d) => {
			const ts = new Date().toISOString();
			return {
				...d,
				phases: (d.phases ?? []).map((p) =>
					p.phase === name
						? { ...p, status: 'running' as const, started_at: ts }
						: p.status === 'running'
							? { ...p, status: 'succeeded' as const, finished_at: ts }
							: p
				)
			};
		});
	const finish = () => {
		const ts = new Date().toISOString();
		const duration = Math.round((Date.parse(ts) - Date.parse(now)) / 1000);
		if (fail) {
			log('error', `Deployment failed: Nothing answered on port ${app.container_port} within 60s`);
			updateDeployment(app.id, id, (d) => ({
				...d,
				status: 'failed',
				finished_at: ts,
				duration_secs: duration,
				phases: (d.phases ?? []).map((p) =>
					p.status === 'running' ? { ...p, status: 'failed' as const, finished_at: ts } : p
				),
				failure: {
					phase: 'health',
					code: 'port_not_listening',
					title: `Nothing answered on port ${app.container_port} within 60s`,
					hint: `Tond expected the app to listen on port ${app.container_port} inside the container. If it uses another port (nginx listens on 80, many Node apps on 3000), change the app's port; make sure the app binds to 0.0.0.0, not localhost.`,
					detail:
						'the new container did not become ready within 60s; the previous version keeps serving',
					excerpt: [
						'/docker-entrypoint.sh: Configuration complete; ready for start up',
						'nginx: [notice] start worker processes'
					]
				}
			}));
			if (appIdx !== -1) apps[appIdx] = { ...apps[appIdx], status: 'failed' };
			return;
		}
		log('info', `Deployed successfully! Container: ${id.slice(4, 16)}`);
		deployments[app.id] = (deployments[app.id] ?? []).map((d) =>
			d.id === id
				? d
				: {
						...d,
						is_current: false,
						rollback_available: d.status === 'succeeded' && !d.image_pruned,
						rollback_unavailable_reason:
							d.status === 'succeeded' && !d.image_pruned ? null : d.rollback_unavailable_reason
					}
		);
		updateDeployment(app.id, id, (d) => ({
			...d,
			status: 'succeeded',
			finished_at: ts,
			duration_secs: duration,
			image_tag: d.image_tag ?? tag,
			is_current: true,
			rollback_unavailable_reason: 'This image is already running',
			phases: (d.phases ?? []).map((p) =>
				p.status === 'running'
					? { ...p, status: 'succeeded' as const, finished_at: ts }
					: p.status === 'pending'
						? { ...p, status: 'skipped' as const }
						: p
			)
		}));
		if (appIdx !== -1) apps[appIdx] = { ...apps[appIdx], status: 'running' };
	};

	const script: [number, () => void][] = [
		[300, () => log('info', opts.rollbackOf ? 'Starting rollback...' : 'Starting deployment...')]
	];
	if (plan === 'git') {
		script.push(
			[400, () => phase('clone')],
			[300, () => log('info', `Cloning ${app.source_url ?? 'repository'} (main)`)],
			[
				1200,
				() => {
					updateDeployment(app.id, id, (d) => ({
						...d,
						commit_sha: '4e5f6a7b8c9d0e1f2a3b4c5d6e7f8091a2b3c4d5',
						commit_message: 'Tidy up the order totals',
						commit_author: 'Ada Lovelace'
					}));
					log('info', 'Commit 4e5f6a7 by Ada Lovelace: Tidy up the order totals');
				}
			],
			[300, () => phase('build')]
		);
		for (const line of mockBuildLog(now, 'Tidy up the order totals', true).slice(4, 15)) {
			script.push([500, () => log(line.level, line.message)]);
		}
		script.push([1500, () => log('info', '#15 exporting to image')]);
	} else if (plan === 'image') {
		script.push(
			[400, () => phase('clone')],
			[300, () => log('info', `Pulling image: ${app.source_url ?? 'image'}`)],
			[2000, () => log('info', `Pinned ${app.source_url} as ${tag}`)]
		);
	} else if (plan !== 'rollback') {
		script.push(
			[400, () => phase('build')],
			[1500, () => log('info', '#1 [internal] load build definition from Dockerfile')],
			[1500, () => log('info', `#6 naming to docker.io/library/${tag} \u001b[32mdone\u001b[0m`)]
		);
	} else {
		script.push([300, () => log('info', `Rolling back to image: ${opts.rollbackOf?.image_tag}`)]);
	}
	script.push(
		[500, () => phase('start')],
		[300, () => log('info', `Starting new container on port 8108:${app.container_port}`)],
		[800, () => phase('health')],
		[
			300,
			() =>
				log(
					'info',
					app.readiness === 'none'
						? `Background worker: waiting ${app.ready_after_secs ?? 10}s to make sure it stays up`
						: app.health_check_path
							? `Waiting up to 30s for the health check on port ${app.container_port}${app.health_check_path} to pass`
							: `Waiting up to 60s for the app to answer on port ${app.container_port}`
				)
		]
	);
	if (fail) {
		script.push([4000, finish]);
	} else {
		script.push(
			[
				2000,
				() =>
					log(
						'info',
						app.readiness === 'none'
							? 'The worker stayed up — replacing the old container'
							: 'New container is ready — cutting over traffic'
					)
			],
			[200, () => phase('release')],
			[
				300,
				() =>
					app.readiness !== 'none' &&
					log('info', 'Waiting 3s for proxy to pick up new port before stopping old container')
			],
			[1500, () => log('info', 'Draining old container 4be1c2a9d0f3 (30s grace)')],
			[800, finish]
		);
	}

	let wait = 0;
	for (const [ms, action] of script) {
		wait += ms;
		setTimeout(action, wait);
	}
	return deployment;
}

function deploymentDetail(app: App, d: Deployment) {
	return {
		...d,
		logs: deploymentLogs[d.id] ?? [],
		logs_truncated: false,
		container_port: app.container_port,
		health_check_path: app.health_check_path,
		source_type: app.source_type
	};
}

function isFinishedStatus(status: string) {
	return ['succeeded', 'failed', 'superseded', 'cancelled'].includes(status);
}

const deploymentStreamHandler = deploymentStream.addEventListener('connection', ({ client }) => {
	const match = client.url.pathname.match(/\/api\/apps\/([^/]+)\/deployments\/([^/]+)\/stream/);
	const app = match ? apps.find((a) => a.id === match[1] || a.name === match[1]) : undefined;
	const d = app ? (deployments[app.id] ?? []).find((x) => x.id === match?.[2]) : undefined;
	if (!app || !d) {
		client.close(4004, 'Deployment not found');
		return;
	}
	const detail = deploymentDetail(app, d);
	client.send(
		JSON.stringify({
			type: 'snapshot',
			deployment: d,
			logs: detail.logs,
			logs_truncated: false,
			container_port: detail.container_port,
			health_check_path: detail.health_check_path,
			source_type: detail.source_type
		})
	);
	if (isFinishedStatus(d.status)) {
		setTimeout(() => client.close(1000, 'Deployment finished'), 50);
		return;
	}
	const listener = (frame: DeployFrame) => {
		client.send(JSON.stringify(frame));
		const dep = frame.deployment as Deployment | undefined;
		if (frame.type === 'deployment' && dep && isFinishedStatus(dep.status)) {
			// Let the last frames go out before closing, as a real socket would.
			setTimeout(() => client.close(1000, 'Deployment finished'), 50);
		}
	};
	const set = deployListeners.get(d.id) ?? new Set();
	set.add(listener);
	deployListeners.set(d.id, set);
	client.addEventListener('close', () => set.delete(listener));
});

/** Mock of GET /api/domains/check (server: handlers/domains.rs). */
function mockDomainCheck(domain: string) {
	const serverIp = '203.0.113.10';
	const parts = domain.split('.');
	const zone = parts.length >= 2 ? parts.slice(-2).join('.') : null;
	const rel = (host: string) =>
		zone && host === zone ? '@' : zone ? host.slice(0, -(zone.length + 1)) : host;
	const appsDomain = MOCK_SETTINGS.apps_domain || null;
	const under = !!appsDomain && domain.endsWith(`.${appsDomain}`);
	const record = (host: string) => ({
		type: 'A' as const,
		name: rel(host),
		host,
		value: serverIp
	});
	const resolved: Record<string, string[]> = {
		'api.example.com': [serverIp],
		'app.example.com': ['198.51.100.23'],
		'db.example.com': [serverIp, '198.51.100.23']
	};
	const ips = under ? [serverIp] : (resolved[domain] ?? []);
	const stray = ips.filter((ip) => ip !== serverIp);
	let status: 'ok' | 'mismatch' | 'missing' | 'inconclusive';
	let message: string;
	if (ips.length === 0) {
		status = 'missing';
		message = `${domain} has no A or AAAA record yet. Create the record below; DNS changes usually show up within a few minutes.`;
	} else if (ips.includes(serverIp)) {
		status = 'ok';
		message = stray.length
			? `${domain} points at this server, but also at ${stray.join(', ')}. Remove that record: visitors and Let's Encrypt may reach the other address.`
			: `${domain} points at this server.`;
	} else {
		status = 'mismatch';
		message = `DNS for ${domain} points to ${ips.join(', ')}, expected ${serverIp}. Update the domain's A/AAAA record to this server.`;
	}
	const cert = certificates.find((c) => c.domain === domain) ?? null;
	return {
		domain,
		zone,
		apps_domain: appsDomain,
		under_apps_domain: under,
		records: [record(domain)],
		wildcard_records: under ? [record(`*.${appsDomain}`)] : [],
		warning: mockDomainWarning(domain),
		server_ips: [serverIp],
		dns: { status, resolved: ips, expected: [serverIp], stray, message },
		certificate: cert
			? {
					status: cert.status,
					issued_at: cert.issued_at,
					expires_at: cert.expires_at,
					last_error: cert.last_error,
					last_attempt_at: cert.last_attempt_at,
					next_retry_at: cert.next_retry_at,
					failure_count: cert.failure_count
				}
			: null,
		checked_at: new Date().toISOString()
	};
}

/** Ports a few well-known images expose (mock of POST /api/images/inspect). */
function mockImagePorts(image: string): { exposed: number[]; env: number | null } {
	const name = image.toLowerCase();
	if (/nginx|httpd|caddy|whoami|apache/.test(name)) return { exposed: [80], env: null };
	if (/grafana|metabase/.test(name)) return { exposed: [3000], env: null };
	if (/ghost/.test(name)) return { exposed: [2368], env: null };
	if (/node|next/.test(name)) return { exposed: [], env: 3000 };
	return { exposed: [], env: null };
}

export const handlers = [
	// Host history, disk, Docker, heartbeat, uptime (./observe.ts)
	...observeHandlers,

	// Scenario gate: runs first and falls through (returns undefined) when idle.
	http.all('/api/*', ({ request }) => {
		const scenario = mockScenario();
		if (!scenario) return;
		const path = new URL(request.url).pathname;
		if (scenario === 'server-down') {
			return new HttpResponse('<html><body><h1>502 Bad Gateway</h1></body></html>', {
				status: 502,
				headers: { 'Content-Type': 'text/html' }
			});
		}
		if (scenario === 'session-expired' && !CREDENTIAL_PATHS.some((p) => path.startsWith(p))) {
			clearMockScenario();
			return err('Unauthorized', 401);
		}
		if (scenario === 'rate-limited' && path === '/api/auth/login') {
			return HttpResponse.json(
				{ success: false, data: null, error: 'Rate limit exceeded. Try again later.' },
				{ status: 429, headers: { 'Retry-After': '30' } }
			);
		}
	}),

	// ── Auth ────────────────────────────────────────────────────────────────

	http.get('/api/auth/status', async () => {
		await delay(LATENCY);
		const scenario = mockScenario();
		if ((scenario === 'logged-out' || scenario === 'mfa') && !mockSignedIn) {
			return ok({ authenticated: false, user: null });
		}
		if (needsSetup) {
			return ok({ authenticated: false, user: null });
		}
		return ok({ authenticated: true, user: currentMockUser() });
	}),

	http.post('/api/auth/setup/check', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { token?: string };
		if (!needsSetup || body.token !== MOCK_SETUP_TOKEN) {
			return err(
				'This setup link is invalid or has already been used. If the admin account already exists, sign in instead.',
				403
			);
		}
		return ok({ valid: true });
	}),

	http.post('/api/auth/login', async () => {
		await delay(LATENCY);
		if (mockScenario() === 'mfa') {
			return ok({ mfa_required: true });
		}
		mockSignedIn = true;
		return ok({ user: currentMockUser() });
	}),

	http.post('/api/auth/logout', async () => {
		await delay(LATENCY);
		return ok(null);
	}),

	http.get('/api/auth/me', async () => {
		await delay(LATENCY);
		return ok(currentMockUser());
	}),

	http.post('/api/auth/setup', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { token?: string };
		if (!needsSetup || body.token !== MOCK_SETUP_TOKEN) {
			return err(
				'This setup link is invalid or has already been used. If the admin account already exists, sign in instead.',
				403
			);
		}
		needsSetup = false;
		return ok({ user: MOCK_USER });
	}),

	// ── Users (users:admin) ───────────────────────────────────────────────────

	http.get('/api/users', async () => {
		await delay(LATENCY);
		return ok(users);
	}),

	http.post('/api/users', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { email?: string; role?: string };
		const email = body.email?.trim() ?? '';
		if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
			return err('Invalid input: Enter a valid email address, like name@example.com.', 400);
		}
		if (users.some((u) => u.email.toLowerCase() === email.toLowerCase())) {
			return err(`App already exists: a user with the email ${email}`, 409);
		}
		const role = (body.role ?? 'viewer') as 'admin' | 'deployer' | 'viewer';
		const newUser = {
			id: `usr_${Date.now()}`,
			email,
			role,
			totp_enabled: false,
			created_at: new Date().toISOString(),
			disabled: false,
			password_set: false,
			last_login_at: null,
			scopes: [],
			active_sessions: 0,
			is_you: false
		};
		users = [...users, newUser];
		return HttpResponse.json(
			{
				success: true,
				data: {
					user: newUser,
					token: MOCK_PASSWORD_LINK,
					path: `/set-password?token=${MOCK_PASSWORD_LINK}`,
					expires_at: new Date(Date.now() + 86400_000).toISOString()
				},
				error: null
			},
			{ status: 201 }
		);
	}),

	http.patch('/api/users/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { role?: string; disabled?: boolean };
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('Not found: user', 404);
		if (target.is_you) {
			return err(
				"Invalid input: You can't change your own role or disable yourself. Ask another admin."
			);
		}
		users = users.map((u) =>
			u.id === params.id
				? {
						...u,
						role: (body.role as typeof u.role) ?? u.role,
						disabled: body.disabled ?? u.disabled,
						active_sessions: 0
					}
				: u
		);
		return ok(users.find((u) => u.id === params.id));
	}),

	http.delete('/api/users/:id', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('Not found: user', 404);
		if (target.is_you)
			return err("Invalid input: You can't delete your own account. Ask another admin.");
		users = users.filter((u) => u.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/users/:id/password-link', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('Not found: user', 404);
		return HttpResponse.json(
			{
				success: true,
				data: {
					user: target,
					token: MOCK_PASSWORD_LINK,
					path: `/set-password?token=${MOCK_PASSWORD_LINK}`,
					expires_at: new Date(Date.now() + 86400_000).toISOString()
				},
				error: null
			},
			{ status: 201 }
		);
	}),

	http.get('/api/users/:id/sessions', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('Not found: user', 404);
		return ok(
			Array.from({ length: target.active_sessions }, (_, i) => ({
				id: `sess_${target.id}_${i}`,
				user_agent:
					i === 0
						? 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/128.0'
						: 'Mozilla/5.0 (iPhone; CPU iPhone OS 17_5) Safari/604.1',
				ip_address: i === 0 ? '198.51.100.7' : '192.0.2.18',
				created_at: new Date(Date.now() - (i + 1) * 5 * 3600_000).toISOString(),
				last_used_at: new Date(Date.now() - (i + 1) * 20 * 60_000).toISOString(),
				expires_at: new Date(Date.now() + 29 * 86400_000).toISOString(),
				idle_expires_at: new Date(Date.now() + 11 * 3600_000).toISOString(),
				is_current: target.is_you && i === 0
			}))
		);
	}),

	http.delete('/api/users/:id/sessions', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('Not found: user', 404);
		const revoked = target.is_you ? target.active_sessions - 1 : target.active_sessions;
		users = users.map((u) =>
			u.id === params.id ? { ...u, active_sessions: u.active_sessions - revoked } : u
		);
		return ok({ revoked });
	}),

	http.delete('/api/users/:id/sessions/:sessionId', async ({ params }) => {
		await delay(LATENCY);
		users = users.map((u) =>
			u.id === params.id ? { ...u, active_sessions: Math.max(0, u.active_sessions - 1) } : u
		);
		return ok({ revoked: 1 });
	}),

	// ── Set-password links (public) ───────────────────────────────────────────

	http.post('/api/auth/password-link/check', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { token?: string };
		if (body.token !== MOCK_PASSWORD_LINK) {
			return err(
				'This link is invalid, has expired or was already used. Ask an admin of this server for a new one.',
				403
			);
		}
		return ok({
			email: 'victor@example.com',
			purpose: 'invite',
			expires_at: new Date(Date.now() + 20 * 3600_000).toISOString()
		});
	}),

	http.post('/api/auth/password-link/complete', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { token?: string; password?: string };
		if (body.token !== MOCK_PASSWORD_LINK) {
			return err(
				'This link is invalid, has expired or was already used. Ask an admin of this server for a new one.',
				403
			);
		}
		const problem = mockPasswordProblem(body.password ?? '');
		if (problem) return err(problem, 400);
		mockSignedIn = true;
		return ok({ signed_in: true, user: currentMockUser() });
	}),

	// ── API tokens ────────────────────────────────────────────────────────────

	http.get('/api/tokens/catalog', async () => {
		await delay(LATENCY);
		return ok(MOCK_TOKEN_CATALOG);
	}),

	http.get('/api/tokens', async () => {
		await delay(LATENCY);
		return ok(apiTokens);
	}),

	http.post('/api/tokens', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as {
			name?: string;
			kind?: string;
			scopes?: string[];
			app_ids?: string[];
			project_ids?: string[];
			allowed_ips?: string[];
			expires_in_hours?: number;
		};
		if (!body.name?.trim()) {
			return err(
				'Invalid input: Give the token a name, e.g. the machine or workflow that uses it.'
			);
		}
		if (!body.scopes?.length) return err('Invalid input: Choose at least one scope or a preset.');
		const hex = Array.from({ length: 64 }, () => Math.floor(Math.random() * 16).toString(16)).join(
			''
		);
		const secret = `tond_pat_${hex}`;
		const hours = body.expires_in_hours ?? 24 * 90;
		const token = {
			id: `tok_${Date.now()}`,
			name: body.name.trim(),
			prefix: secret.slice(0, 17),
			kind: (body.kind ?? 'personal') as 'personal' | 'service' | 'agent' | 'ci',
			scopes: body.scopes,
			app_ids: body.app_ids ?? [],
			project_ids: body.project_ids ?? [],
			allowed_ips: body.allowed_ips ?? [],
			owner_id: MOCK_USER.id,
			owner_label: MOCK_USER.email,
			expires_at: hours === 0 ? null : new Date(Date.now() + hours * 3600_000).toISOString(),
			last_used_at: null,
			last_used_ip: null,
			revoked_at: null,
			created_at: new Date().toISOString()
		};
		apiTokens = [token, ...apiTokens];
		return HttpResponse.json(
			{ success: true, data: { token, secret }, error: null },
			{ status: 201 }
		);
	}),

	http.delete('/api/tokens/:id', async ({ params }) => {
		await delay(LATENCY);
		const token = apiTokens.find((t) => t.id === params.id);
		if (!token) return err('Not found: token', 404);
		const updated = { ...token, revoked_at: token.revoked_at ?? new Date().toISOString() };
		apiTokens = apiTokens.map((t) => (t.id === params.id ? updated : t));
		return ok(updated);
	}),

	http.get('/api/terminal/targets', async () => {
		await delay(LATENCY);
		return ok([
			{
				id: 'server',
				kind: 'server',
				label: 'Server',
				shell_user: 'tond',
				enabled: mockHostTerminalEnabled(),
				requires_step_up: true
			},
			...apps
				.filter((app) => app.status === 'running')
				.slice(0, 4)
				.map((app) => ({
					id: app.id,
					kind: 'app',
					label: `App: ${app.name}`,
					shell_user: null,
					enabled: true,
					requires_step_up: false
				})),
			...databases
				.filter((db) => db.status === 'running')
				.slice(0, 4)
				.map((db) => ({
					id: db.id,
					kind: 'database',
					label: `Database: ${db.name}`,
					shell_user: null,
					enabled: true,
					requires_step_up: false
				}))
		]);
	}),

	http.post('/api/terminal/tickets', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as {
			target_kind: string;
			target_id: string;
			password?: string;
			code?: string;
		};
		if (body.target_kind === 'server') {
			if (!mockHostTerminalEnabled()) {
				return err(
					'The host terminal is turned off. To turn it on, run `sudo tond settings host-terminal on` on the server.',
					403
				);
			}
			if (!body.password) {
				return err('Confirm your password to open the host terminal', 401);
			}
			if (body.password !== MOCK_STEP_UP_PASSWORD) {
				return err('Password is incorrect', 401);
			}
		}
		const ticket = `mock-ticket-${Date.now()}-${Math.random().toString(16).slice(2)}`;
		terminalTickets.set(ticket, { kind: body.target_kind, id: body.target_id });
		return HttpResponse.json(
			{ success: true, data: { ticket, expires_in: 30 }, error: null },
			{ status: 201 }
		);
	}),

	http.get('/api/settings/host-terminal', async () => {
		await delay(LATENCY);
		return ok({ enabled: mockHostTerminalEnabled() });
	}),

	http.put('/api/settings/host-terminal', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { enabled?: boolean };
		return err(
			`The host terminal can only be switched on or off on the server itself: run \`sudo tond settings host-terminal ${body.enabled ? 'on' : 'off'}\`.`,
			403
		);
	}),

	// ── Phase 7.1 — Password Management ─────────────────────────────────────

	http.post('/api/auth/change-password', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as {
			current_password: string;
			new_password: string;
		};
		if (body.current_password === 'wrong') {
			return err('Current password is incorrect', 401);
		}
		const problem = mockPasswordProblem(body.new_password ?? '');
		if (problem) return err(problem, 400);
		return ok({ message: 'Password changed successfully' });
	}),

	http.get('/api/auth/sessions', async () => {
		await delay(LATENCY);
		return ok([
			{
				id: 'sess_current',
				user_agent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36',
				ip_address: '127.0.0.1',
				created_at: new Date(Date.now() - 3600_000).toISOString(),
				last_used_at: new Date().toISOString(),
				expires_at: new Date(Date.now() + 29 * 86400_000).toISOString(),
				idle_expires_at: new Date(Date.now() + 12 * 3600_000).toISOString(),
				is_current: true
			},
			{
				id: 'sess_other',
				user_agent: 'tond-cli/0.1.0',
				ip_address: '192.168.1.42',
				created_at: new Date(Date.now() - 86400_000).toISOString(),
				last_used_at: new Date(Date.now() - 7200_000).toISOString(),
				expires_at: new Date(Date.now() + 20 * 86400_000).toISOString(),
				idle_expires_at: new Date(Date.now() + 10 * 3600_000).toISOString(),
				is_current: false
			}
		]);
	}),

	http.delete('/api/auth/sessions/:id', async () => {
		await delay(LATENCY);
		return ok({ message: 'Session revoked' });
	}),

	http.delete('/api/auth/sessions', async () => {
		await delay(LATENCY);
		return ok({ message: 'Revoked 1 session(s)' });
	}),

	// ── Phase 7.1 — TOTP 2FA ────────────────────────────────────────────────

	http.post('/api/auth/2fa/setup', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { password?: string };
		if (!body.password || body.password === 'wrong') {
			return err('Password is incorrect', 401);
		}
		if (mockTotpEnabled) {
			return err('Two-factor authentication is already enabled', 409);
		}
		const secret = 'JBSWY3DPEHPK3PXP';
		// Minimal 1×1 white PNG as a stand-in for the real server-generated QR code
		const qr_code =
			'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwADhQGAWjR9awAAAABJRU5ErkJggg==';
		return ok({
			secret,
			otpauth_uri: `otpauth://totp/Tond:admin@example.com?secret=${secret}&issuer=Tond&algorithm=SHA1&digits=6&period=30`,
			qr_code
		});
	}),

	http.post('/api/auth/2fa/confirm', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { code: string };
		if (body.code !== '123456') {
			return err('Invalid TOTP code', 401);
		}
		mockTotpEnabled = true;
		return ok({
			message: '2FA enabled successfully',
			backup_codes: Array.from({ length: 10 }, () =>
				Math.random().toString(16).slice(2, 18).padEnd(16, '0')
			)
		});
	}),

	http.post('/api/auth/2fa/disable', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { password: string; code: string };
		if (body.password === 'wrong') {
			return err('Current password is incorrect', 401);
		}
		if (body.code !== '123456') {
			return err('Invalid TOTP or backup code', 401);
		}
		mockTotpEnabled = false;
		return ok({ message: '2FA disabled' });
	}),

	http.post('/api/auth/2fa/verify', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { code: string };
		if (body.code !== '123456') {
			return err('Invalid TOTP or backup code', 401);
		}
		mockSignedIn = true;
		return ok({ user: currentMockUser() });
	}),

	// ── Projects ──────────────────────────────────────────────────────────────

	http.get('/api/projects', async () => {
		await delay(LATENCY);
		const withCounts = projects.map((p) => ({
			...p,
			active_preview_count: (projectPreviews[p.id] ?? []).filter(
				(pp) => pp.status !== 'expired' && pp.status !== 'failed'
			).length
		}));
		return ok(withCounts);
	}),

	http.get('/api/templates', async () => {
		await delay(LATENCY);
		return ok({
			templates,
			memory: MOCK_TEMPLATE_MEMORY,
			apps_domain: MOCK_SETTINGS.apps_domain
		});
	}),

	http.post('/api/templates/:id/deploy', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const template = templates.find((t) => t.id === params.id);
		if (!template) return err(`Not found: No template named ${params.id}`, 404);
		const body = (await request.json()) as TemplateDeployRequest;
		const name = (body.name ?? '').trim().toLowerCase();
		if (!/^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/.test(name)) {
			return err('Invalid input: App name must be 1–40 lowercase letters, digits and hyphens');
		}
		if (apps.some((a) => a.name === name)) return err(`App already exists: ${name}`, 409);
		if (template.ram_mb > MOCK_TEMPLATE_MEMORY.available_mb && !body.allow_low_memory) {
			return err(
				`${template.name} needs about ${template.ram_mb} MB of memory and this server has ${MOCK_TEMPLATE_MEMORY.available_mb} MB free. Stop or remove an app to make room, or deploy anyway and let it swap.`,
				409,
				'insufficient_memory'
			);
		}
		const credentials: { key: string; label: string; value: string }[] = [];
		for (const input of template.inputs) {
			const given = (body.inputs?.[input.key] ?? '').trim();
			if (!given && input.required) return err(`Invalid input: ${input.label} is required`);
			if (given && input.kind === 'email' && !given.includes('@')) {
				return err(`Invalid input: ${input.label} must be an email address`);
			}
			const value = given || (input.generated ? mockGeneratedSecret() : (input.default ?? ''));
			if (input.credential) credentials.push({ key: input.key, label: input.label, value });
		}
		const domain =
			body.domain?.trim() ||
			(MOCK_SETTINGS.apps_domain ? `${name}.${MOCK_SETTINGS.apps_domain}` : null);
		const now = new Date().toISOString();
		const newApp: App = {
			...structuredClone(MOCK_APPS[0]),
			id: `app_${Date.now()}`,
			name,
			domain,
			source_type: 'compose',
			source_url: null,
			status: 'deploying',
			container_port: template.public.port,
			host_port: null,
			health_check_path: template.public.health_path,
			volumes: [],
			build_method: null,
			auto_deploy: false,
			watch_branch: null,
			project_id: body.project_id ?? null,
			compose_content: template.compose,
			dockerfile_content: null,
			readiness: template.public.health_path ? 'http' : 'tcp',
			created_at: now,
			updated_at: now
		};
		apps = [...apps, newApp];
		const d = startMockDeploy(newApp, {
			trigger_source: 'manual',
			trigger_detail: MOCK_USER.email
		});
		return HttpResponse.json(
			{
				success: true,
				data: {
					app_id: newApp.id,
					app_name: name,
					domain,
					deployment_id: d.id,
					credentials,
					setup_notes: template.setup_notes
				},
				error: null
			},
			{ status: 202 }
		);
	}),

	http.post('/api/projects', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			description?: string;
			env_vars?: Record<string, string>;
		};
		const newProjectId = `proj_${Date.now()}`;
		projectEnv[newProjectId] = body.env_vars ?? {};
		const newProject: Project = {
			id: newProjectId,
			name: body.name,
			description: body.description ?? null,
			env: maskedEntries(body.env_vars ?? {}),
			active_preview_count: 0,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		projects = [...projects, newProject];
		return ok(newProject);
	}),

	http.get('/api/projects/:id', async ({ params }) => {
		await delay(LATENCY);
		const project = projects.find((p) => p.id === params.id || p.name === params.id);
		if (!project) return err('Project not found', 404);
		const memberApps = apps.filter((a) => a.project_id === project.id);
		return ok({ ...project, apps: memberApps });
	}),

	http.put('/api/projects/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = projects.findIndex((p) => p.id === params.id || p.name === params.id);
		if (idx === -1) return err('Project not found', 404);
		const { env_vars, ...body } = (await request.json()) as Partial<
			Pick<Project, 'name' | 'description'> & { env_vars: Record<string, string> }
		>;
		if (env_vars) projectEnv[projects[idx].id] = env_vars;
		projects[idx] = {
			...projects[idx],
			...body,
			...(env_vars && { env: maskedEntries(env_vars) }),
			updated_at: new Date().toISOString()
		};
		return ok(projects[idx]);
	}),

	http.delete('/api/projects/:id', async ({ params }) => {
		await delay(LATENCY);
		const project = projects.find((p) => p.id === params.id || p.name === params.id);
		if (project) {
			// Un-assign member apps
			apps = apps.map((a) => (a.project_id === project.id ? { ...a, project_id: null } : a));
		}
		projects = projects.filter((p) => p.id !== params.id && p.name !== params.id);
		return ok(null);
	}),

	http.post('/api/projects/:id/deploy', async ({ params }) => {
		await delay(LATENCY * 3);
		const project = projects.find((p) => p.id === params.id || p.name === params.id);
		if (!project) return err('Project not found', 404);
		// Mark all member apps as running
		apps = apps.map((a) =>
			a.project_id === project.id
				? { ...a, status: 'running', updated_at: new Date().toISOString() }
				: a
		);
		return ok(null);
	}),

	// PUT /api/apps/:id/project — assign / un-assign an app from a project
	http.put('/api/apps/:id/project', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { project_id: string | null };
		apps[idx] = { ...apps[idx], project_id: body.project_id, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Apps ─────────────────────────────────────────────────────────────────

	http.get('/api/apps', async () => {
		await delay(LATENCY);
		return ok(apps);
	}),

	http.post('/api/apps', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as Omit<Partial<App>, 'volumes'> & {
			volumes?: VolumeInput[];
		};
		if (body.source_type === 'compose') {
			const problems = mockComposeProblems(body.compose_content ?? '');
			if (problems.length === 1) return err(`Compose file rejected: ${problems[0]}`, 400);
			if (problems.length > 1) {
				return err(
					`Compose file rejected (${problems.length} problems):\n- ${problems.join('\n- ')}`,
					400
				);
			}
		}
		const appName = body.name ?? 'new-app';
		for (const v of body.volumes ?? []) {
			const problem = mockVolumeError(v, `${MOCK_VOLUMES_DIR}/${appName}`);
			if (problem) return err(problem, 400);
		}
		if (body.readiness === 'none' && body.domain) {
			return err(
				'Invalid input: Background workers have no domain: they serve no traffic. Leave the domain empty.'
			);
		}
		const newApp: App = {
			id: `app_${Date.now()}`,
			name: body.name ?? 'new-app',
			domain: body.domain ?? null,
			source_type: body.source_type ?? 'git',
			source_url: body.source_url ?? null,
			status: 'stopped',
			container_port: body.container_port ?? 3000,
			host_port: null,
			memory_limit: body.memory_limit ?? null,
			cpu_limit: body.cpu_limit ?? null,
			health_check_path: null,
			health_check_interval: 30,
			health_failure_threshold: 3,
			health_consecutive_failures: 0,
			volumes: (body.volumes ?? []).map((v) => mockAppVolume(appName, v)),
			drain_timeout: 30,
			build_method:
				body.source_type === 'git' || body.source_type === 'dockerfile'
					? (body.build_method ?? (body.source_type === 'dockerfile' ? 'dockerfile' : 'tbuild'))
					: null,
			auto_deploy: false,
			watch_branch: null,
			deploy_key_pub: null,
			project_id: null,
			registry_credential_id: null,
			github_app_id: body.github_app_id ?? null,
			dockerfile_content: body.dockerfile_content ?? null,
			compose_content: body.compose_content ?? null,
			base_dir: body.base_dir ?? null,
			static_build: body.static_build ?? false,
			preview_enabled: false,
			preview_shared_keys: [],
			readiness: body.readiness ?? 'http',
			ready_after_secs: body.ready_after_secs ?? 10,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		apps = [...apps, newApp];
		return ok(newApp);
	}),

	http.get('/api/apps/:id', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		return ok({ ...app, domain_warning: mockDomainWarning(app.domain) });
	}),

	http.get('/api/apps/:id/databases', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		if (app.source_type === 'compose') return ok([]);
		const toAccess = (db: ManagedDatabase, via: 'project' | 'link') => ({
			id: db.id,
			name: db.name,
			db_type: db.db_type,
			status: db.status,
			via,
			internal_host: db.name,
			internal_port: DB_PORTS[db.db_type] ?? 0
		});
		const viaProject = app.project_id
			? databases.filter((d) => d.project_id === app.project_id).map((d) => toAccess(d, 'project'))
			: [];
		const viaLink = databaseLinks
			.filter((l) => l.app_id === app.id)
			.map((l) => databases.find((d) => d.id === l.database_id))
			.filter((d): d is ManagedDatabase => !!d && !viaProject.some((p) => p.id === d.id))
			.map((d) => toAccess(d, 'link'));
		return ok([...viaProject, ...viaLink]);
	}),

	http.delete('/api/apps/:id', async ({ params }) => {
		await delay(LATENCY);
		apps = apps.filter((a) => a.id !== params.id);
		databaseLinks = databaseLinks.filter((l) => l.app_id !== params.id);
		return ok(null);
	}),

	http.post('/api/apps/:id/deploy', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const d = startMockDeploy(app, { trigger_source: 'manual', trigger_detail: MOCK_USER.email });
		return HttpResponse.json(
			{
				success: true,
				data: {
					message: 'Deployment queued',
					app_id: app.id,
					deployment_id: d.id,
					status: 'queued',
					// The mock never has to wait: the build queue is always free.
					queue_position: 1,
					superseded: null
				},
				error: null
			},
			{ status: 202 }
		);
	}),

	http.put('/api/apps/:id/readiness', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { readiness: string; ready_after_secs?: number };
		if (!['http', 'tcp', 'none'].includes(body.readiness)) {
			return err('Invalid input: readiness must be one of: http, tcp, none');
		}
		const readiness = body.readiness as 'http' | 'tcp' | 'none';
		apps[idx] = {
			...apps[idx],
			readiness,
			ready_after_secs: body.ready_after_secs ?? apps[idx].ready_after_secs ?? 10,
			...(readiness === 'none' ? { domain: null, health_check_path: null } : {}),
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	http.put('/api/apps/:id/port', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { container_port: number };
		const port = Number(body.container_port);
		if (!Number.isInteger(port) || port < 1 || port > 65535) {
			return err('Invalid input: container_port must be between 1 and 65535');
		}
		apps[idx] = { ...apps[idx], container_port: port, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Source inspection and domain checks ──

	http.post('/api/git/branches', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as { url: string; github_app_id?: string };
		if (!/^https?:\/\//.test(body.url ?? '')) {
			return err(
				'Invalid input: Branches can only be listed for http(s) Git URLs; type the branch name instead'
			);
		}
		if (/private/.test(body.url) && !body.github_app_id) {
			return err(
				"Invalid input: Tond couldn't read this repository. If it's private, pick a connected GitHub App or add credentials; you can also type the branch name."
			);
		}
		const trunk = /legacy/.test(body.url) ? 'master' : 'main';
		return ok({
			default_branch: trunk,
			branches: [trunk, 'develop', 'feature/checkout', 'release/2.4'],
			truncated: false
		});
	}),

	http.post('/api/images/inspect', async ({ request }) => {
		await delay(LATENCY * 4);
		const body = (await request.json()) as { image: string; registry_credential_id?: string };
		const image = (body.image ?? '').trim();
		if (!image || /\s/.test(image)) {
			return err(
				'Invalid input: Enter an image reference like nginx:alpine or ghcr.io/org/app:1.2'
			);
		}
		if (/does-not-exist|private/.test(image) && !body.registry_credential_id) {
			// Soft failure, as the server does for a pull the registry refuses.
			return ok({
				image,
				exposed_ports: [],
				env_port: null,
				suggested_port: null,
				pulled: false,
				unavailable: `Tond couldn't read ${image}. If it's private, choose registry credentials on the Environment step and Tond reads its port then; otherwise check the name and tag.`,
				needs_credentials: true
			});
		}
		const { exposed, env } = mockImagePorts(image);
		const suggested =
			env && (exposed.length === 0 || exposed.includes(env)) ? env : (exposed[0] ?? null);
		return ok({
			image,
			exposed_ports: exposed,
			env_port: env,
			suggested_port: suggested,
			pulled: true
		});
	}),

	http.get('/api/domains/check', async ({ request }) => {
		await delay(LATENCY * 2);
		const domain = (new URL(request.url).searchParams.get('domain') ?? '').trim().toLowerCase();
		if (!/^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*$/.test(domain)) {
			return err(`Invalid input: '${domain}' isn't a valid domain name`);
		}
		return ok(mockDomainCheck(domain));
	}),

	http.post('/api/apps/:id/stop', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = {
			...apps[idx],
			status: 'stopped',
			host_port: null,
			updated_at: new Date().toISOString()
		};
		return ok(null);
	}),

	http.post('/api/apps/:id/start', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = { ...apps[idx], status: 'running', updated_at: new Date().toISOString() };
		return ok(null);
	}),

	http.post('/api/apps/:id/restart', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = { ...apps[idx], status: 'running', updated_at: new Date().toISOString() };
		return ok(null);
	}),

	// ── App environment (masked reads, per-key patches) ──

	http.get('/api/apps/:id/env', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		return ok(appEnvResponse(app));
	}),

	http.get('/api/apps/:id/env/:key', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const value = envStore(app.id)[params.key as string];
		if (value === undefined) return err(`Not found: variable '${params.key}'`, 404);
		return ok({ key: params.key, value });
	}),

	http.patch('/api/apps/:id/env', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const body = (await request.json()) as { set?: Record<string, string>; unset?: string[] };
		const problem = validateEnvPatch(body);
		if (problem) return err(problem);
		appEnvVars[app.id] = applyEnvPatch(envStore(app.id), body);
		return ok(appEnvResponse(app));
	}),

	http.get('/api/apps/:id/preview-env/:key', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const value = previewStore(app.id)[params.key as string];
		if (value === undefined) return err(`Not found: preview override '${params.key}'`, 404);
		return ok({ key: params.key, value });
	}),

	http.patch('/api/apps/:id/preview-env', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as {
			set?: Record<string, string>;
			unset?: string[];
			share?: string[];
			unshare?: string[];
		};
		const hasSharing = (body.share?.length ?? 0) + (body.unshare?.length ?? 0) > 0;
		const problem = hasSharing && !body.set && !body.unset ? null : validateEnvPatch(body);
		if (problem) return err(problem);
		const app = apps[idx];
		previewEnvVars[app.id] = applyEnvPatch(previewStore(app.id), body);
		const shared = new Set(app.preview_shared_keys);
		for (const key of body.share ?? []) shared.add(key);
		for (const key of body.unshare ?? []) shared.delete(key);
		apps[idx] = { ...app, preview_shared_keys: [...shared].sort() };
		return ok(appEnvResponse(apps[idx]));
	}),

	http.put('/api/apps/:id/domain', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { domain: string };
		const domain = normalizeDomain(body.domain);
		if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(domain)) {
			return err(
				`Invalid input: '${domain}' is not a valid domain name (expected something like app.example.com)`
			);
		}
		if (domain === MOCK_SETTINGS.dashboard_domain) {
			return err(
				`Invalid input: ${domain} is the dashboard domain; choose another hostname for the app`
			);
		}
		const preview = MOCK_SETTINGS.preview_domain;
		if (preview && (domain === preview || domain.endsWith(`.${preview}`))) {
			return err(
				`Invalid input: ${domain} is under the preview domain (${preview}), which only serves previews; choose a hostname outside it`
			);
		}
		apps[idx] = { ...apps[idx], domain, updated_at: new Date().toISOString() };
		queueMockCertificate(domain);
		return ok({
			message: 'Domain updated, certificate provisioning queued',
			domain,
			warning: mockDomainWarning(domain)
		});
	}),

	// ── Certificates ──────────────────────────────────────────────────────────

	http.get('/api/certs', async () => {
		await delay(LATENCY);
		return ok(certificates);
	}),

	http.post('/api/certs/:domain/retry', async ({ params }) => {
		await delay(LATENCY);
		const cert = certificates.find((c) => c.domain === params.domain);
		if (!cert) return err(`No certificate for ${params.domain}`, 404);
		cert.next_retry_at = null;
		cert.last_attempt_at = new Date().toISOString();
		return ok({ message: 'Certificate retry queued', domain: cert.domain });
	}),

	http.put('/api/apps/:id/health', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as {
			health_check_path: string | null;
			health_check_interval: number;
			health_failure_threshold: number;
		};
		apps[idx] = { ...apps[idx], ...body, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	http.put('/api/apps/:id/drain-timeout', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { drain_timeout: number };
		apps[idx] = {
			...apps[idx],
			drain_timeout: body.drain_timeout,
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	http.patch('/api/apps/:id/resources', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as {
			memory_limit: number | null;
			cpu_limit: number | null;
		};
		apps[idx] = {
			...apps[idx],
			memory_limit: body.memory_limit,
			cpu_limit: body.cpu_limit,
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	http.post('/api/apps/:id/volumes', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as VolumeInput & { host_path?: string };
		const problem = mockVolumeError(body, `${MOCK_VOLUMES_DIR}/${apps[idx].name}`);
		if (problem) return err(problem, 400);
		if (apps[idx].volumes.some((v) => v.container_path === body.container_path)) {
			return err(`A volume is already mounted at ${body.container_path}`, 400);
		}
		const vol = mockAppVolume(apps[idx].name, body);
		apps[idx] = {
			...apps[idx],
			volumes: [...apps[idx].volumes, vol],
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	http.delete('/api/apps/:id/volumes', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { container_path: string };
		apps[idx] = {
			...apps[idx],
			volumes: apps[idx].volumes.filter((v) => v.container_path !== body.container_path),
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	// ── Deployments ───────────────────────────────────────────────────────────

	http.get('/api/apps/:id/deployments/stats', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const outcomes = Array.from({ length: 30 }, (_, i) => {
			const status =
				i === 7 || i === 19 || i === 20 ? 'failed' : i === 12 ? 'superseded' : 'succeeded';
			return {
				id: `dep_trend_${i}`,
				status,
				started_at: new Date(Date.now() - (30 - i) * 86_400_000).toISOString(),
				duration_secs: 60 + ((i * 17) % 50),
				commit_sha: (0xabc1234 + i).toString(16)
			};
		});
		const succeeded = outcomes.filter((o) => o.status === 'succeeded').length;
		const failed = outcomes.filter((o) => o.status === 'failed').length;
		const durations = outcomes
			.filter((o) => o.status === 'succeeded')
			.map((o) => o.duration_secs)
			.sort((a, b) => a - b);
		return ok({
			window: outcomes.length,
			succeeded,
			failed,
			success_rate: succeeded / (succeeded + failed),
			median_duration_secs: durations[Math.floor(durations.length / 2)],
			outcomes
		});
	}),

	http.get('/api/apps/:id/deployments', async ({ params }) => {
		await delay(LATENCY);
		return ok(deployments[params.id as string] ?? []);
	}),

	http.get('/api/apps/:id/deployments/:deploymentId/generated-dockerfile', async ({ params }) => {
		await delay(LATENCY);
		const appDeployments = deployments[params.id as string] ?? [];
		const deployment = appDeployments.find((d) => d.id === params.deploymentId);
		if (!deployment || !deployment.has_generated_dockerfile) {
			return err('Generated Dockerfile not found', 404);
		}

		const generatedDockerfile = MOCK_GENERATED_DOCKERFILES[params.deploymentId as string];
		if (!generatedDockerfile) {
			return err('Generated Dockerfile not found', 404);
		}

		return ok({
			deployment_id: params.deploymentId,
			generated_dockerfile: generatedDockerfile
		});
	}),

	http.post('/api/apps/:id/rollback', async ({ params, request }) => {
		await delay(LATENCY);
		const text = await request.text();
		const body = (text.trim() ? JSON.parse(text) : {}) as { deployment_id?: string };
		const list = deployments[params.id as string] ?? [];
		const target = body.deployment_id
			? list.find((d) => d.id === body.deployment_id)
			: list.find((d) => d.rollback_available);
		if (!target) {
			return err(
				'No previous deployment to roll back to: there is no earlier successful deployment whose image is still available',
				400
			);
		}
		if (!target.rollback_available) {
			return err(
				`Can't roll back to this deployment: ${target.rollback_unavailable_reason ?? 'unavailable'}`,
				400
			);
		}
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const d = startMockDeploy(app, {
			trigger_source: 'rollback',
			trigger_detail: MOCK_USER.email,
			rollbackOf: target
		});
		return HttpResponse.json(
			{
				success: true,
				data: {
					message: 'Rollback queued',
					app_id: app.id,
					image_tag: target.image_tag,
					target_deployment_id: target.id,
					deployment_id: d.id,
					status: 'queued',
					queue_position: 1,
					superseded: null
				},
				error: null
			},
			{ status: 202 }
		);
	}),

	http.get('/api/apps/:id/deployments/:deploymentId', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const d = (deployments[app.id] ?? []).find((x) => x.id === params.deploymentId);
		if (!d) return err(`Not found: deployment '${params.deploymentId}'`, 404);
		return ok(deploymentDetail(app, d));
	}),

	http.post('/api/compose/validate', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { compose_content: string };
		const errors = mockComposeProblems(body.compose_content);
		return ok({ valid: errors.length === 0, errors });
	}),

	// ── Git Integration ───────────────────────────────────────────────────────

	http.get('/api/apps/:id/deploy-key', async ({ params }) => {
		await delay(LATENCY);
		const key = MOCK_DEPLOY_KEYS[params.id as string];
		if (!key) return err('No deploy key found', 404);
		return ok(key);
	}),

	http.post('/api/apps/:id/deploy-key', async ({ params }) => {
		await delay(LATENCY);
		const newKey = {
			app_id: params.id as string,
			public_key: `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA${Date.now()} tond@deploy`
		};
		MOCK_DEPLOY_KEYS[params.id as string] = newKey;
		return ok(newKey);
	}),

	http.get('/api/apps/:id/webhook', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		return ok(webhookConfig(app));
	}),

	http.put('/api/apps/:id/webhook-secret', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const body = (await request.json()) as { secret?: unknown };
		const secret = typeof body.secret === 'string' ? body.secret.trim() : '';
		if (secret.length < 16) {
			return err(
				'Invalid input: webhook secret must be at least 16 characters (rotate it to have one generated)',
				400
			);
		}
		webhookSecrets[app.id] = secret;
		return ok(webhookConfig(app));
	}),

	http.get('/api/apps/:id/webhook-secret', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		webhookConfig(app);
		return ok({ secret: webhookSecrets[app.id] });
	}),

	http.post('/api/apps/:id/webhook-secret/rotate', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		webhookSecrets[app.id] = randomHex(32);
		return ok({ secret: webhookSecrets[app.id] });
	}),

	http.put('/api/apps/:id/auto-deploy', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { auto_deploy: boolean; watch_branch: string | null };
		apps[idx] = { ...apps[idx], ...body, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	http.put('/api/apps/:id/source', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { source_type: string; source_url: string | null };
		apps[idx] = { ...apps[idx], ...body, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Registry Credentials (Phase 4.3) ──────────────────────────────────────

	http.get('/api/registry-credentials', async () => {
		await delay(LATENCY);
		return ok(registryCredentials);
	}),

	http.post('/api/registry-credentials', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			server: string;
			username: string;
			password: string;
		};
		const newCred: RegistryCredential = {
			id: `cred_${Date.now()}`,
			name: body.name,
			server: body.server,
			username: body.username,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		registryCredentials = [...registryCredentials, newCred];
		return ok(newCred);
	}),

	http.delete('/api/registry-credentials/:id', async ({ params }) => {
		await delay(LATENCY);
		registryCredentials = registryCredentials.filter((c) => c.id !== params.id);
		// Clear from any apps that had this credential
		apps = apps.map((a) =>
			a.registry_credential_id === params.id
				? { ...a, registry_credential_id: null, updated_at: new Date().toISOString() }
				: a
		);
		return ok(null);
	}),

	http.put('/api/apps/:id/registry-credential', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { registry_credential_id: string | null };
		apps[idx] = {
			...apps[idx],
			registry_credential_id: body.registry_credential_id,
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	// ── Preview Deployments ───────────────────────────────────────────────────

	http.get('/api/apps/:id/previews', async ({ params }) => {
		await delay(LATENCY);
		return ok(previews[params.id as string] ?? []);
	}),

	http.post('/api/apps/:id/preview', async ({ params, request }) => {
		await delay(LATENCY);
		if (!MOCK_SETTINGS.preview_domain) return err(PREVIEW_DOMAIN_REQUIRED);
		const app = apps.find((a) => a.id === params.id);
		const body = (await request.json()) as {
			ttl_hours?: number;
			pr_number?: number;
			head_sha?: string;
			head_ref?: string;
			pr_url?: string;
		};
		// Mirrors the server: stored PR links must be https:// URLs.
		if (body.pr_url && !/^https:\/\/[^/@\s]+/i.test(body.pr_url)) {
			return err('Invalid input: pr_url must be an https:// URL', 400);
		}
		const ttl = (body.ttl_hours ?? 24) * 3600;
		const slug = `preview-${Date.now()}`;
		const newPreview: PreviewDeployment = {
			id: `prev_${Date.now()}`,
			app_id: params.id as string,
			project_preview_id: null,
			slug,
			subdomain: `${slug}-${app?.name ?? 'app'}.${MOCK_SETTINGS.preview_domain}`,
			container_id: null,
			host_port: null,
			image_tag: null,
			ttl_seconds: ttl,
			status: 'building',
			expires_at: new Date(Date.now() + ttl * 1000).toISOString(),
			created_at: new Date().toISOString(),
			url: `https://${slug}-${app?.name ?? 'app'}.${MOCK_SETTINGS.preview_domain}`,
			pr_number: body.pr_number ?? null,
			head_sha: body.head_sha ?? null,
			head_ref: body.head_ref ?? null,
			pr_url: body.pr_url ?? null,
			comment_id: null
		};
		previews = {
			...previews,
			[params.id as string]: [...(previews[params.id as string] ?? []), newPreview]
		};
		return ok(newPreview);
	}),

	http.delete('/api/apps/:id/previews/:previewId', async ({ params }) => {
		await delay(LATENCY);
		const appId = params.id as string;
		previews = {
			...previews,
			[appId]: (previews[appId] ?? []).filter((p) => p.id !== params.previewId)
		};
		return ok(null);
	}),

	http.put('/api/apps/:id/preview-config', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { preview_enabled: boolean };
		if (body.preview_enabled && !MOCK_SETTINGS.preview_domain) {
			return err(PREVIEW_DOMAIN_REQUIRED);
		}
		apps[idx] = {
			...apps[idx],
			preview_enabled: body.preview_enabled,
			updated_at: new Date().toISOString()
		};
		return ok({ preview_enabled: body.preview_enabled });
	}),

	// ── Project Previews ──────────────────────────────────────────────────────

	http.get('/api/projects/:id/previews', async ({ params }) => {
		await delay(LATENCY);
		const projectId = params.id as string;
		return ok(projectPreviews[projectId] ?? []);
	}),

	http.post('/api/projects/:id/previews', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const projectId = params.id as string;
		const project = projects.find((p) => p.id === projectId || p.name === projectId);
		if (!project) return err('Project not found', 404);
		if (!MOCK_SETTINGS.preview_domain) return err(PREVIEW_DOMAIN_REQUIRED);
		const body = (await request.json()) as { ttl_hours?: number };
		const ttlHours = body.ttl_hours ?? 24;
		const ttlSeconds = ttlHours * 3600;
		const slug = `prev-${Date.now().toString(36)}`;
		const expiresAt = new Date(Date.now() + ttlSeconds * 1000).toISOString();
		const createdAt = new Date().toISOString();
		// Build per-app previews for all member apps
		const memberApps = apps.filter((a) => a.project_id === project.id);
		const childPreviews: PreviewDeployment[] = memberApps.map((app, i) => ({
			id: `prev_${Date.now()}_${i}`,
			app_id: app.id,
			project_preview_id: `pprev_${Date.now()}`,
			slug,
			subdomain: `${slug}-${app.name}.${MOCK_SETTINGS.preview_domain}`,
			container_id: null,
			host_port: null,
			image_tag: null,
			ttl_seconds: ttlSeconds,
			status: 'running',
			expires_at: expiresAt,
			created_at: createdAt,
			url: `https://${slug}-${app.name}.${MOCK_SETTINGS.preview_domain}`,
			pr_number: null,
			head_sha: null,
			head_ref: null,
			pr_url: null,
			comment_id: null
		}));
		const newProjectPreview: ProjectPreview = {
			id: `pprev_${Date.now()}`,
			project_id: project.id,
			slug,
			ttl_seconds: ttlSeconds,
			status: memberApps.length === 0 ? 'failed' : 'running',
			expires_at: expiresAt,
			created_at: createdAt,
			previews: childPreviews
		};
		// Fix child preview project_preview_id to match parent
		for (const child of childPreviews) {
			child.project_preview_id = newProjectPreview.id;
		}
		projectPreviews = {
			...projectPreviews,
			[project.id]: [newProjectPreview, ...(projectPreviews[project.id] ?? [])]
		};
		return ok(newProjectPreview);
	}),

	http.delete('/api/projects/:id/previews/:previewId', async ({ params }) => {
		await delay(LATENCY);
		const projectId = params.id as string;
		const list = projectPreviews[projectId] ?? [];
		const exists = list.some((pp) => pp.id === params.previewId);
		if (!exists) return err('Project preview not found', 404);
		projectPreviews = {
			...projectPreviews,
			[projectId]: list.filter((pp) => pp.id !== params.previewId)
		};
		return ok(null);
	}),

	// ── Metrics ───────────────────────────────────────────────────────────────

	// Stopped / never-deployed apps have no samples (shows the empty state).
	http.get('/api/apps/:id/metrics', async ({ request, params }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '1h';
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		const measured = app?.status === 'running' || app?.status === 'unhealthy';
		return ok(makeMockMetrics(range, !measured));
	}),

	http.get('/api/databases/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h'));
	}),

	http.get('/api/cron/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h', true));
	}),

	http.get('/api/previews/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h'));
	}),

	// ── Request metrics (tond-proxy traffic) ──────────────────────────────────

	// Apps that are not running (and workers without a domain) have no traffic.
	http.get('/api/apps/:id/requests', async ({ request, params }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '24h';
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const served = (app.status === 'running' || app.status === 'unhealthy') && !!app.domain;
		return ok(makeMockRequestMetrics(range, { empty: !served }));
	}),

	http.get('/api/previews/:id/requests', async ({ request }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '24h';
		return ok(makeMockRequestMetrics(range, { scale: 0.3, withDeploys: false }));
	}),

	http.get('/api/system/requests', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockTrafficOverview(new URL(request.url).searchParams.get('range') ?? '24h'));
	}),

	// ── Access logs ───────────────────────────────────────────────────────────

	http.get('/api/apps/:id/access-log', async ({ request, params }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const limit = Number(url.searchParams.get('lines') ?? 500);
		const q = (url.searchParams.get('q') ?? '').toLowerCase();
		const log = accessLogs[params.id as string] ?? {
			enabled: false,
			include_query: false,
			lines: []
		};
		const matching = q ? log.lines.filter((l) => l.toLowerCase().includes(q)) : log.lines;
		return ok({
			enabled: log.enabled,
			include_query: log.include_query,
			size_bytes: log.lines.reduce((sum, l) => sum + l.length + 1, 0),
			max_bytes: 60 * 1024 * 1024,
			lines: matching.slice(-limit),
			truncated: matching.length > limit
		});
	}),

	http.put('/api/apps/:id/access-log', async ({ request, params }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { enabled: boolean; include_query?: boolean };
		const id = params.id as string;
		const current = accessLogs[id] ?? { enabled: false, include_query: false, lines: [] };
		accessLogs = {
			...accessLogs,
			[id]: {
				...current,
				enabled: body.enabled,
				include_query: body.enabled && !!body.include_query
			}
		};
		return ok({ enabled: accessLogs[id].enabled, include_query: accessLogs[id].include_query });
	}),

	http.delete('/api/apps/:id/access-log', async ({ params }) => {
		await delay(LATENCY);
		const id = params.id as string;
		if (accessLogs[id]) accessLogs = { ...accessLogs, [id]: { ...accessLogs[id], lines: [] } };
		return ok(null);
	}),

	http.get('/api/apps/:id/access-log/download', async ({ params }) => {
		const lines = accessLogs[params.id as string]?.lines ?? [];
		return new HttpResponse(lines.map((l) => `${l}\n`).join(''), {
			headers: {
				'Content-Type': 'text/plain; charset=utf-8',
				'Content-Disposition': 'attachment; filename="access.log"'
			}
		});
	}),

	http.get('/api/system/self', async () => {
		await delay(LATENCY);
		return ok(makeMockSystemSelf());
	}),

	http.get('/api/system/metrics', async () => {
		await delay(LATENCY);
		return ok(makeMockSystemMetrics());
	}),

	// ── Settings ──────────────────────────────────────────────────────────────

	http.get('/api/settings', async () => {
		await delay(LATENCY);
		return ok(MOCK_SETTINGS);
	}),

	http.get('/api/onboarding/status', async () => {
		await delay(LATENCY);
		MOCK_ONBOARDING_STATUS.dashboard_domain = MOCK_SETTINGS.dashboard_domain;
		MOCK_ONBOARDING_STATUS.has_projects = projects.length > 0;
		MOCK_ONBOARDING_STATUS.has_apps = apps.length > 0;
		MOCK_ONBOARDING_STATUS.has_databases = databases.length > 0;
		MOCK_ONBOARDING_STATUS.has_cron_jobs = cronJobs.length > 0;
		MOCK_ONBOARDING_STATUS.has_source_integration =
			githubApps.some((g) => g.installed) || registryCredentials.length > 0;
		const allDeployments = Object.values(deployments)
			.flat()
			.sort((a, b) => b.created_at.localeCompare(a.created_at));
		MOCK_ONBOARDING_STATUS.has_successful_deploy = allDeployments.some(
			(d) => d.status === 'succeeded'
		);
		MOCK_ONBOARDING_STATUS.has_notification_channel = integrations.length > 0;
		const latest = allDeployments[0];
		MOCK_ONBOARDING_STATUS.last_deployment = latest
			? {
					id: latest.id,
					app_id: latest.app_id,
					app_name: apps.find((a) => a.id === latest.app_id)?.name ?? latest.app_id,
					status: latest.status,
					trigger_source: latest.trigger_source,
					created_at: latest.created_at,
					finished_at: latest.finished_at
				}
			: null;
		MOCK_ONBOARDING_STATUS.is_fresh_install =
			!MOCK_ONBOARDING_STATUS.has_projects &&
			!MOCK_ONBOARDING_STATUS.has_apps &&
			!MOCK_ONBOARDING_STATUS.has_databases &&
			!MOCK_ONBOARDING_STATUS.has_cron_jobs;
		MOCK_ONBOARDING_STATUS.domain_status.configured =
			MOCK_SETTINGS.dashboard_domain.trim().length > 0;
		MOCK_ONBOARDING_STATUS.domain_status.expected_target = MOCK_ONBOARDING_STATUS.server_public_ip;
		MOCK_ONBOARDING_STATUS.domain_status.dns_matches_server = MOCK_SETTINGS.dashboard_domain.trim()
			? true
			: null;
		MOCK_ONBOARDING_STATUS.domain_status.https_ready = MOCK_SETTINGS.dashboard_domain.trim()
			? false
			: null;
		MOCK_ONBOARDING_STATUS.domain_status.certificate_status = MOCK_SETTINGS.dashboard_domain.trim()
			? 'pending'
			: null;
		return ok(MOCK_ONBOARDING_STATUS);
	}),

	http.get('/api/apps/:id/events', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const url = new URL(request.url);
		const limit = Number(url.searchParams.get('limit') ?? 50);
		const before = url.searchParams.get('before');
		const all = makeMockAppEvents(app.id).filter((e) => !before || e.at < before);
		const page = all.slice(0, limit);
		return ok({
			events: page,
			next_before: all.length > limit ? page[page.length - 1].at : null
		});
	}),

	http.get('/api/audit-log/shipping', async () => {
		await delay(LATENCY);
		return ok(MOCK_AUDIT_SHIPPING);
	}),

	http.put('/api/audit-log/shipping', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as UpdateAuditShipping;
		if (body.syslog) {
			if (body.syslog.enabled && !body.syslog.host.trim()) {
				return err("Enter the syslog server's host name or IP address.", 400);
			}
			const repointed =
				body.syslog.host !== MOCK_AUDIT_SHIPPING.syslog.host ||
				body.syslog.port !== MOCK_AUDIT_SHIPPING.syslog.port;
			Object.assign(MOCK_AUDIT_SHIPPING.syslog, body.syslog);
			if (repointed) {
				MOCK_AUDIT_SHIPPING.syslog.status.shipped_seq = 0;
				MOCK_AUDIT_SHIPPING.syslog.status.pending = MOCK_AUDIT_SHIPPING.latest_seq;
			}
		}
		if (body.webhook) {
			const url = body.webhook.url.trim();
			if (url && !url.startsWith('https://')) return err('The webhook must use https://.', 400);
			const secret = body.webhook.secret?.trim();
			if (secret && secret.length < 16) {
				return err('The signing secret must be at least 16 characters.', 400);
			}
			if (body.webhook.enabled && !secret && !MOCK_AUDIT_SHIPPING.webhook.has_secret) {
				return err(
					'Set a signing secret so the receiver can check each delivery came from this server.',
					400
				);
			}
			MOCK_AUDIT_SHIPPING.webhook.enabled = body.webhook.enabled;
			MOCK_AUDIT_SHIPPING.webhook.url = url;
			if (secret) MOCK_AUDIT_SHIPPING.webhook.has_secret = true;
		}
		return ok(MOCK_AUDIT_SHIPPING);
	}),

	http.post('/api/audit-log/shipping/test', async ({ request }) => {
		await delay(LATENCY * 2);
		const { destination } = (await request.json()) as { destination: string };
		if (destination === 'syslog') {
			return ok({
				ok: true,
				message: 'Test entry sent. Look for action audit_shipping_test on the syslog server.'
			});
		}
		if (!MOCK_AUDIT_SHIPPING.webhook.url)
			return ok({ ok: false, message: 'Save a webhook URL first.' });
		return ok({ ok: false, message: 'the webhook answered HTTP 401' });
	}),

	http.get('/api/settings/retention', async () => {
		await delay(LATENCY);
		return ok(MOCK_RETENTION);
	}),

	http.put('/api/settings/retention', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as Record<string, number>;
		for (const [key, value] of Object.entries(body)) {
			const field = MOCK_RETENTION.fields.find((f) => f.key === key);
			if (!field) return err(`Unknown setting '${key}'`, 400);
			if (!Number.isInteger(value) || value < field.min || value > field.max) {
				return err(`${key} must be between ${field.min} and ${field.max} ${field.unit}`, 400);
			}
			field.value = value;
		}
		return ok(MOCK_RETENTION);
	}),

	http.put('/api/settings', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as Partial<typeof MOCK_SETTINGS>;
		if (body.dashboard_domain !== undefined) {
			body.dashboard_domain = normalizeDomain(body.dashboard_domain);
		}
		if (body.preview_domain !== undefined) {
			body.preview_domain = normalizeDomain(body.preview_domain);
		}
		if (body.apps_domain !== undefined) {
			body.apps_domain = normalizeDomain(body.apps_domain);
		}
		const previewError = mockPreviewDomainError(
			body.preview_domain ?? MOCK_SETTINGS.preview_domain,
			body.dashboard_domain ?? MOCK_SETTINGS.dashboard_domain
		);
		if (previewError) return err(previewError);
		const appsError = mockAppsDomainError(
			body.apps_domain ?? MOCK_SETTINGS.apps_domain,
			body.dashboard_domain ?? MOCK_SETTINGS.dashboard_domain,
			body.preview_domain ?? MOCK_SETTINGS.preview_domain
		);
		if (appsError) return err(appsError);
		if (
			body.upload_max_bytes !== undefined &&
			(body.upload_max_bytes < 1024 * 1024 || body.upload_max_bytes > 64 * 1024 ** 3)
		) {
			return err('The upload limit must be between 1.0 MiB and 64.0 GiB');
		}
		Object.assign(MOCK_SETTINGS, body);
		if (body.dashboard_domain) queueMockCertificate(body.dashboard_domain);
		MOCK_ONBOARDING_STATUS.dashboard_domain = MOCK_SETTINGS.dashboard_domain;
		MOCK_ONBOARDING_STATUS.domain_status.configured =
			MOCK_SETTINGS.dashboard_domain.trim().length > 0;
		MOCK_ONBOARDING_STATUS.domain_status.dns_matches_server = MOCK_SETTINGS.dashboard_domain.trim()
			? true
			: null;
		MOCK_ONBOARDING_STATUS.domain_status.https_ready = MOCK_SETTINGS.dashboard_domain.trim()
			? false
			: null;
		MOCK_ONBOARDING_STATUS.domain_status.certificate_status = MOCK_SETTINGS.dashboard_domain.trim()
			? 'pending'
			: null;
		return ok(MOCK_SETTINGS);
	}),

	// ── Alerts ────────────────────────────────────────────────────────────────

	http.get('/api/alerts/kinds', async () => {
		await delay(LATENCY);
		return ok(MOCK_ALERT_KINDS);
	}),

	http.get('/api/alerts/active', async () => {
		await delay(LATENCY);
		return ok(activeAlerts.filter((a) => alertRules.find((r) => r.id === a.rule_id)?.enabled));
	}),

	http.get('/api/alerts/history', async ({ request }) => {
		await delay(LATENCY);
		const p = new URL(request.url).searchParams;
		const perPage = Math.min(Math.max(Number(p.get('per_page') ?? 25) || 25, 1), 100);
		const pageNo = Math.max(Number(p.get('page') ?? 1) || 1, 1);
		const matching = alertHistory.filter(
			(e) =>
				(!p.get('rule_id') || e.rule_id === p.get('rule_id')) &&
				(!p.get('app_id') || e.app_id === p.get('app_id')) &&
				(!p.get('state') || e.state === p.get('state'))
		);
		return ok({
			episodes: matching.slice((pageNo - 1) * perPage, pageNo * perPage),
			total: matching.length
		});
	}),

	http.get('/api/alerts/history/:id', async ({ params }) => {
		await delay(LATENCY);
		const ep = alertHistory.find((e) => e.id === params.id);
		if (!ep) return err('Alert not found', 404);
		const rule = alertRules.find((r) => r.id === ep.rule_id);
		const deliveries =
			MOCK_ALERT_DELIVERIES[ep.id] ??
			(rule?.integration_ids ?? []).flatMap((id, i) => {
				const channel = integrations.find((c) => c.id === id);
				if (!channel) return [];
				const events: ('firing' | 'resolved')[] = ep.resolved_at
					? ['firing', 'resolved']
					: ['firing'];
				return events.map((event) => ({
					id: `d_${ep.id}_${i}_${event}`,
					episode_id: ep.id,
					integration_id: id,
					integration_name: channel.name,
					integration_kind: channel.kind,
					event,
					status: 'sent' as const,
					attempts: 1,
					deferred: false,
					last_error: null,
					next_attempt_at: null,
					sent_at: event === 'firing' ? ep.fired_at : ep.resolved_at,
					created_at: event === 'firing' ? ep.fired_at : (ep.resolved_at ?? ep.fired_at)
				}));
			});
		return ok({ episode: ep, deliveries });
	}),

	http.post('/api/alerts/history/:id/acknowledge', async ({ params }) => {
		await delay(LATENCY);
		const ep = alertHistory.find((e) => e.id === params.id && e.state === 'firing');
		if (!ep) return err("That alert isn't firing any more.", 404);
		const now = new Date().toISOString();
		ep.acknowledged_at = now;
		ep.acknowledged_by = 'admin@example.com';
		for (const a of activeAlerts) {
			if (a.episode_id === ep.id) {
				a.acknowledged_at = now;
				a.acknowledged_by = 'admin@example.com';
			}
		}
		return ok({ acknowledged: ep.id });
	}),

	http.get('/api/alerts/settings', async () => {
		await delay(LATENCY);
		return ok(alertSettings);
	}),

	http.put('/api/alerts/settings', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as AlertSettings;
		const q = body.quiet_hours;
		if (!/^\d{2}:\d{2}$/.test(q.start) || !/^\d{2}:\d{2}$/.test(q.end)) {
			return err('Quiet hours need a start and end time like 22:00.', 400);
		}
		if (q.start === q.end) return err('Quiet hours need different start and end times.', 400);
		if (!q.timezone.includes('/') && q.timezone !== 'UTC') {
			return err(`Unknown time zone '${q.timezone}'. Use a name like Europe/Berlin.`, 400);
		}
		alertSettings = body;
		return ok(alertSettings);
	}),

	http.get('/api/alerts', async () => {
		await delay(LATENCY);
		return ok(alertRules);
	}),

	http.get('/api/alerts/:id', async ({ params }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		return ok(rule);
	}),

	http.post('/api/alerts', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as AlertRuleInput;
		const kind = MOCK_ALERT_KINDS.find((k) => k.kind === body.kind);
		if (!kind) return err(`Unknown rule kind '${body.kind}'.`, 400);
		if (!kind.available) return err(kind.unavailable_reason ?? 'Not available', 400);
		if (!body.name?.trim()) return err('Give the rule a name.', 400);
		const channelIds = [...new Set(body.integration_ids ?? [])];
		if (channelIds.length === 0) {
			return err('Choose at least one notification channel, so this rule notifies someone.', 400);
		}
		for (const id of channelIds) {
			if (!integrations.some((i) => i.id === id)) {
				return err(`Notification channel ${id} doesn't exist.`, 400);
			}
		}
		const appId = kind.scope === 'app' ? (body.app_id ?? null) : null;
		const now = new Date().toISOString();
		const newRule: AlertRule = {
			id: `alert_${Date.now()}`,
			name: body.name.trim(),
			kind: body.kind,
			app_id: appId,
			app_name: appId ? (apps.find((a) => a.id === appId)?.name ?? null) : null,
			threshold: kind.default_threshold === null ? 0 : (body.threshold ?? kind.default_threshold),
			window_secs:
				kind.default_window_secs === null ? 0 : (body.window_secs ?? kind.default_window_secs),
			for_secs: body.for_secs ?? kind.default_for_secs,
			severity: body.severity ?? kind.default_severity,
			repeat_secs: body.repeat_secs ?? 14_400,
			enabled: true,
			snoozed_until: null,
			integration_ids: channelIds.sort(),
			firing: 0,
			pending: 0,
			last_fired_at: null,
			created_at: now,
			updated_at: now
		};
		alertRules = [...alertRules, newRule];
		return HttpResponse.json({ success: true, data: newRule, error: null }, { status: 201 });
	}),

	http.patch('/api/alerts/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = alertRules.findIndex((r) => r.id === params.id);
		if (idx === -1) return err('Alert rule not found', 404);
		const body = (await request.json()) as AlertRuleUpdate;
		if (body.integration_ids && body.integration_ids.length === 0) {
			return err('Choose at least one notification channel, so this rule notifies someone.', 400);
		}
		const { snoozed_until, ...rest } = body;
		alertRules[idx] = {
			...alertRules[idx],
			...rest,
			...(snoozed_until !== undefined ? { snoozed_until: snoozed_until || null } : {}),
			updated_at: new Date().toISOString()
		};
		if (body.enabled === false) {
			activeAlerts = activeAlerts.filter((a) => a.rule_id !== params.id);
			alertRules[idx] = { ...alertRules[idx], firing: 0, pending: 0 };
		}
		return ok(alertRules[idx]);
	}),

	http.post('/api/alerts/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		const results = rule.integration_ids.map((id) => {
			const integration = integrations.find((i) => i.id === id);
			const enabled = integration?.enabled ?? false;
			return {
				integration_id: id,
				kind: integration?.kind ?? 'webhook',
				name: integration?.name ?? id,
				ok: enabled,
				error: enabled ? null : 'This channel is switched off on the Integrations page'
			};
		});
		return ok(results);
	}),

	http.post('/api/alerts/:id/snooze', async ({ params, request }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		const body = (await request.json()) as {
			target?: string;
			minutes?: number;
			until_resolved?: boolean;
		};
		const until = body.minutes ? new Date(Date.now() + body.minutes * 60_000).toISOString() : null;
		if (!body.target) {
			rule.snoozed_until = until;
		} else {
			for (const a of activeAlerts) {
				if (a.rule_id === rule.id && a.target === body.target) {
					a.snoozed_until = body.until_resolved ? null : until;
					a.snoozed_until_resolved = !!body.until_resolved;
				}
			}
		}
		return ok({ snoozed_until: until, until_resolved: !!body.until_resolved });
	}),

	http.post('/api/alerts/:id/unsnooze', async ({ params, request }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		const body = (await request.json()) as { target?: string };
		if (!body.target) rule.snoozed_until = null;
		for (const a of activeAlerts) {
			if (a.rule_id === rule.id && (!body.target || a.target === body.target)) {
				a.snoozed_until = null;
				a.snoozed_until_resolved = false;
			}
		}
		return ok({ unsnoozed: true });
	}),

	http.delete('/api/alerts/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = alertRules.some((r) => r.id === params.id);
		if (!exists) return err('Alert rule not found', 404);
		alertRules = alertRules.filter((r) => r.id !== params.id);
		activeAlerts = activeAlerts.filter((a) => a.rule_id !== params.id);
		return ok({ deleted: params.id });
	}),

	// ── Audit Log ─────────────────────────────────────────────────────────────

	// Same filters as the server, applied before the limit.
	http.get('/api/audit-log', async ({ request }) => {
		await delay(LATENCY);
		const p = new URL(request.url).searchParams;
		const limit = Math.min(Math.max(parseInt(p.get('limit') ?? '50', 10) || 50, 1), 200);
		const before = p.get('before') ? Number(p.get('before')) : null;
		const actor = p.get('actor');
		const action = p.get('action');
		const resourceType = p.get('resource_type');
		const resourceId = p.get('resource_id');
		const outcome = p.get('outcome');
		const q = p.get('q')?.toLowerCase();
		const matches = auditLog.filter(
			(e) =>
				(before === null || e.seq < before) &&
				(!actor || e.actor_label === actor) &&
				(!action ||
					(action.endsWith('*')
						? e.action.startsWith(action.slice(0, -1))
						: e.action === action)) &&
				(!resourceType || e.resource_type === resourceType) &&
				(!resourceId || e.resource_id === resourceId) &&
				(!outcome || e.outcome === outcome) &&
				(!q ||
					[e.actor_label, e.action, e.resource_name ?? '', e.detail ?? ''].some((f) =>
						f.toLowerCase().includes(q)
					))
		);
		const entries = matches.slice(0, limit);
		const nextBefore = matches.length > limit ? entries[entries.length - 1].seq : null;
		return ok({ entries, next_before: nextBefore });
	}),

	http.get('/api/audit-log/verify', async () => {
		await delay(LATENCY * 2);
		const seqs = auditLog.map((e) => e.seq);
		return ok({
			ok: true,
			checked: auditLog.length,
			checkpoint_seq: Math.min(...seqs) - 1,
			first_seq: Math.min(...seqs),
			last_seq: Math.max(...seqs),
			broken_at: null,
			problem: null
		});
	}),

	// ── App Logs (REST fallback — real traffic uses WebSocket) ────────────────

	http.get('/api/apps/:id/logs', async () => {
		await delay(LATENCY);
		return ok({
			deploy_logs: [
				{
					level: 'info',
					trigger_source: 'manual',
					message: '[mock] Build started',
					created_at: new Date().toISOString()
				},
				{
					level: 'info',
					trigger_source: 'manual',
					message: '[mock] Installing dependencies…',
					created_at: new Date().toISOString()
				},
				{
					level: 'info',
					trigger_source: 'manual',
					message: '[mock] Build complete',
					created_at: new Date().toISOString()
				}
			],
			container_logs: [
				{ ts: new Date().toISOString(), line: '[mock] Server listening on :8080' },
				{ ts: new Date().toISOString(), line: '[mock] GET /healthz 200 OK' },
				{ ts: new Date().toISOString(), line: '[mock] GET /api/users 200 OK' }
			]
		});
	}),

	// ── Managed Databases (Phase 6.3) ─────────────────────────────────────────
	// Passwords and connection-string overrides are write-only, like the
	// server: responses carry has_password flags and masked strings, and the
	// plaintext comes only from the (audited) credentials reveal.

	http.get('/api/databases', async () => {
		await delay(LATENCY);
		return ok(databases);
	}),

	http.post('/api/databases', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			db_type: string;
			project_id?: string;
			memory_limit?: number;
			public_access?: boolean;
		};
		const defaultImages: Record<string, string> = {
			postgres: 'postgres:16-alpine',
			mysql: 'mysql:8',
			redis: 'redis:7-alpine',
			mariadb: 'mariadb:11',
			keydb: 'eqalpha/keydb:latest',
			mongodb: 'mongo:7'
		};
		const isRedisLike = body.db_type === 'redis' || body.db_type === 'keydb';
		const isMysqlLike = body.db_type === 'mysql' || body.db_type === 'mariadb';
		const newDb: ManagedDatabase = {
			id: `db_${Date.now()}`,
			name: body.name,
			db_type: body.db_type,
			project_id: body.project_id ?? null,
			host_port: 5400 + databases.length + 1,
			status: 'running',
			db_name: isRedisLike ? null : body.name.replace(/-/g, '_'),
			db_user: isRedisLike ? 'default' : body.name.replace(/-/g, '_') + '_user',
			has_password: true,
			has_root_password: isMysqlLike,
			memory_limit: body.memory_limit ?? null,
			custom_external_url: null,
			custom_internal_url: null,
			image_used: defaultImages[body.db_type] ?? `${body.db_type}:latest`,
			custom_image: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString(),
			public_access: body.public_access ?? false
		};
		dbSecrets[newDb.id] = {
			db_password: randomHex(12),
			root_password: isMysqlLike ? randomHex(12) : null
		};
		databases = [...databases, newDb];
		// The creator gets the generated credentials once.
		return ok({ ...dbDetail(newDb), credentials: dbCredentials(newDb) });
	}),

	http.get('/api/databases/:id/access', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		return ok(mockDatabaseAccess(db));
	}),

	http.put('/api/databases/:id/links/:app', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Not found: database', 404);
		const app = apps.find((a) => a.id === params.app || a.name === params.app);
		if (!app) return err('App not found', 404);
		if (app.source_type === 'compose') {
			return err(
				`Invalid input: '${app.name}' is a compose app: compose apps run on their own compose network and can't be linked.`
			);
		}
		if (db.project_id && db.project_id === app.project_id) {
			return err(
				`Invalid input: '${app.name}' is in the same project as '${db.name}' and can already reach it.`
			);
		}
		if (!databaseLinks.some((l) => l.database_id === db.id && l.app_id === app.id)) {
			databaseLinks = [...databaseLinks, { database_id: db.id, app_id: app.id }];
		}
		return ok(mockDatabaseAccess(db));
	}),

	http.delete('/api/databases/:id/links/:app', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Not found: database', 404);
		const app = apps.find((a) => a.id === params.app || a.name === params.app);
		if (!app) return err('App not found', 404);
		databaseLinks = databaseLinks.filter((l) => !(l.database_id === db.id && l.app_id === app.id));
		return ok(mockDatabaseAccess(db));
	}),

	http.get('/api/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		return ok(dbDetail(db));
	}),

	http.get('/api/databases/:id/credentials', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		if (currentMockUser().role !== 'admin') {
			return err('Forbidden: only admins can reveal database credentials', 403);
		}
		return ok(dbCredentials(db));
	}),

	http.get('/api/databases/:id/backups', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err(`database '${params.id}' not found`, 404);
		return ok(snapshots.filter((s) => s.scope === 'full' || s.scope === `database:${db.name}`));
	}),

	http.post('/api/databases/:id/backups', async ({ params }) => {
		await delay(LATENCY * 4);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err(`database '${params.id}' not found`, 404);
		const snap = mockDatabaseSnapshot(db.name, 'manual');
		snapshots = [snap, ...snapshots];
		return HttpResponse.json({ success: true, data: snap, error: null }, { status: 201 });
	}),

	http.delete('/api/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		const gone = databases.find((d) => d.id === params.id || d.name === params.id);
		databases = databases.filter((d) => d.id !== params.id && d.name !== params.id);
		if (gone) databaseLinks = databaseLinks.filter((l) => l.database_id !== gone.id);
		return ok(null);
	}),

	http.patch('/api/databases/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const body = (await request.json()) as {
			custom_external_url?: string | null;
			custom_internal_url?: string | null;
			custom_image?: string | null;
			memory_limit?: number | null;
			public_access?: boolean;
		};
		// Apply updates — empty string clears the override (mirrors backend behaviour)
		const updated = { ...databases[idx] };
		const urls = (dbCustomUrls[updated.id] ??= { external: null, internal: null });
		const clean = (v: string | null) => (v === null || v.trim() === '' ? null : v.trim());
		if (body.custom_external_url !== undefined) {
			urls.external = clean(body.custom_external_url);
			updated.custom_external_url = urls.external && maskUrlPassword(urls.external);
		}
		if (body.custom_internal_url !== undefined) {
			urls.internal = clean(body.custom_internal_url);
			updated.custom_internal_url = urls.internal && maskUrlPassword(urls.internal);
		}
		if (body.custom_image !== undefined) {
			updated.custom_image =
				body.custom_image === '' || body.custom_image === null ? null : body.custom_image;
		}
		if (body.memory_limit !== undefined) {
			updated.memory_limit = body.memory_limit ?? null;
		}
		if (body.public_access !== undefined) {
			updated.public_access = body.public_access;
		}
		updated.updated_at = new Date().toISOString();
		databases[idx] = updated;
		return ok(dbDetail(updated));
	}),

	http.patch('/api/databases/:id/credentials', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const body = (await request.json()) as {
			db_name?: string | null;
			db_user?: string | null;
			db_password?: string | null;
			root_password?: string | null;
		};
		// Mirrors the server: passwords change inside the running engine; renames,
		// Redis/KeyDB and stopped databases are refused and nothing is saved.
		const current = databases[idx];
		const secrets = (dbSecrets[current.id] ??= { db_password: null, root_password: null });
		const renamed = (next: string | null | undefined, now: string | null) =>
			!!next?.trim() && next.trim() !== now;
		if (renamed(body.db_name, current.db_name) || renamed(body.db_user, current.db_user)) {
			return err(
				"Invalid input: The username of an existing database can't be changed; only passwords can."
			);
		}
		const changed: string[] = [];
		const nextPw = body.db_password?.trim();
		if (nextPw && nextPw !== secrets.db_password) changed.push('db_password');
		const nextRoot = body.root_password?.trim();
		if (nextRoot && nextRoot !== secrets.root_password) changed.push('root_password');
		if (changed.length === 0) return err('Invalid input: Nothing to change: enter a new password.');
		if (current.db_type === 'redis' || current.db_type === 'keydb') {
			return err(
				"Invalid input: Redis and KeyDB passwords can't be changed in place yet. The stored password was not changed."
			);
		}
		if (current.status !== 'running') {
			return err(
				`Invalid input: Start '${current.name}' first: the password is changed inside the running database. Nothing was changed.`
			);
		}
		await delay(LATENCY * 3); // simulate the docker exec
		if (changed.includes('db_password')) secrets.db_password = nextPw!;
		if (changed.includes('root_password')) secrets.root_password = nextRoot!;
		const updated = {
			...current,
			has_password: !!secrets.db_password,
			has_root_password: !!secrets.root_password,
			updated_at: new Date().toISOString()
		};
		databases[idx] = updated;
		return ok({ database: dbDetail(updated), changed });
	}),

	http.post('/api/databases/:id/stop', async ({ params }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		databases[idx] = {
			...databases[idx],
			status: 'stopped',
			host_port: null,
			updated_at: new Date().toISOString()
		};
		return ok(null);
	}),

	http.post('/api/databases/:id/start', async ({ params }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		databases[idx] = {
			...databases[idx],
			status: 'running',
			host_port: 5400 + idx + 1,
			updated_at: new Date().toISOString()
		};
		return ok(null);
	}),

	http.post('/api/databases/:id/redeploy', async ({ params }) => {
		await delay(LATENCY * 3);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const db = databases[idx];
		const defaultImages: Record<string, string> = {
			postgres: 'postgres:16-alpine',
			mysql: 'mysql:8',
			redis: 'redis:7-alpine',
			mariadb: 'mariadb:11',
			keydb: 'eqalpha/keydb:latest',
			mongodb: 'mongo:7'
		};
		const newImage = db.custom_image ?? defaultImages[db.db_type] ?? `${db.db_type}:latest`;
		const updated = {
			...db,
			status: 'running',
			host_port: db.host_port ?? 5400 + idx + 1,
			image_used: newImage,
			updated_at: new Date().toISOString()
		};
		databases[idx] = updated;
		return ok(dbDetail(updated));
	}),

	// ── Cron Jobs (Phase 6.4) ──────────────────────────────────────────────────

	http.get('/api/cron', async () => {
		await delay(LATENCY);
		return ok(cronJobs);
	}),

	http.get('/api/cron/schedule-preview', async ({ request }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const schedule = url.searchParams.get('schedule') ?? '';
		const timezone = url.searchParams.get('timezone') || 'UTC';
		const count = Number(url.searchParams.get('count') ?? 5);
		const runs = mockNextRuns(schedule, count);
		if (!runs) return err(`Invalid cron expression '${schedule}'`, 400);
		return ok({ next_runs: runs, timezone });
	}),

	http.post('/api/cron', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			schedule: string;
			image: string;
			command?: string[];
			project_id?: string;
			timeout_seconds?: number | null;
			env?: Record<string, string>;
			concurrency_policy?: string;
			memory_mb?: number | null;
			cpu_shares?: number | null;
			timezone?: string | null;
			max_retries?: number;
			retry_delay_seconds?: number;
			volumes?: CronVolumeInput[] | null;
		};
		const newJob: CronJob = {
			id: `cron_${Date.now()}`,
			name: body.name,
			schedule: body.schedule,
			image: body.image,
			command: body.command ? JSON.stringify(body.command) : null,
			project_id: body.project_id ?? null,
			enabled: true,
			last_run_at: null,
			next_run_at: new Date(Date.now() + 3_600_000).toISOString(),
			status: 'idle',
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString(),
			timeout_seconds: body.timeout_seconds ?? null,
			env: maskedEntries(body.env ?? {}),
			last_run_status: null,
			concurrency_policy: body.concurrency_policy ?? 'skip',
			memory_mb: body.memory_mb ?? null,
			cpu_shares: body.cpu_shares ?? null,
			timezone: body.timezone ?? null,
			max_retries: body.max_retries ?? 0,
			retry_delay_seconds: body.retry_delay_seconds ?? 60,
			volumes: mockCronVolumes(`cron_${Date.now()}`, body.volumes)
		};
		cronJobs = [...cronJobs, newJob];
		cronEnv[newJob.id] = body.env ?? {};
		cronRuns[newJob.id] = [];
		return ok(newJob);
	}),

	http.get('/api/cron/:id', async ({ params }) => {
		await delay(LATENCY);
		const job = cronJobs.find((j) => j.id === params.id || j.name === params.id);
		if (!job) return err('Cron job not found', 404);
		return ok(job);
	}),

	http.patch('/api/cron/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = cronJobs.findIndex((j) => j.id === params.id || j.name === params.id);
		if (idx === -1) return err('Cron job not found', 404);
		const body = (await request.json()) as Partial<{
			name: string;
			project_id: string | null;
			schedule: string;
			image: string;
			command: string[];
			enabled: boolean;
			timeout_seconds: number | null;
			env: Record<string, string>;
			env_set: Record<string, string>;
			env_unset: string[];
			concurrency_policy: string;
			memory_mb: number | null;
			cpu_shares: number | null;
			timezone: string | null;
			max_retries: number;
			retry_delay_seconds: number;
			volumes: CronVolumeInput[] | null;
		}>;
		// Env: full replace, then set/unset patches (values stay write-only)
		const jobId = cronJobs[idx].id;
		const nextEnv = { ...(body.env ?? cronEnv[jobId] ?? {}), ...(body.env_set ?? {}) };
		for (const key of body.env_unset ?? []) delete nextEnv[key];
		cronEnv[jobId] = nextEnv;
		// Check for name conflict
		if (body.name !== undefined && body.name !== cronJobs[idx].name) {
			const conflict = cronJobs.some((j, i) => i !== idx && j.name === body.name);
			if (conflict) return err(`A cron job named "${body.name}" already exists`, 409);
		}
		cronJobs[idx] = {
			...cronJobs[idx],
			...(body.name !== undefined && { name: body.name }),
			...('project_id' in body && { project_id: body.project_id ?? null }),
			...(body.schedule !== undefined && { schedule: body.schedule }),
			...(body.image !== undefined && { image: body.image }),
			...(body.command !== undefined && { command: JSON.stringify(body.command) }),
			...(body.enabled !== undefined && { enabled: body.enabled }),
			...('timeout_seconds' in body && { timeout_seconds: body.timeout_seconds ?? null }),
			env: maskedEntries(nextEnv),
			...(body.concurrency_policy !== undefined && {
				concurrency_policy: body.concurrency_policy
			}),
			...('memory_mb' in body && { memory_mb: body.memory_mb ?? null }),
			...('cpu_shares' in body && { cpu_shares: body.cpu_shares ?? null }),
			...('timezone' in body && { timezone: body.timezone ?? null }),
			...(body.max_retries !== undefined && { max_retries: body.max_retries }),
			...(body.retry_delay_seconds !== undefined && {
				retry_delay_seconds: body.retry_delay_seconds
			}),
			...('volumes' in body && {
				volumes: mockCronVolumes(cronJobs[idx].id, body.volumes)
			}),
			updated_at: new Date().toISOString()
		};
		return ok(cronJobs[idx]);
	}),

	http.delete('/api/cron/:id', async ({ params }) => {
		await delay(LATENCY);
		cronJobs = cronJobs.filter((j) => j.id !== params.id && j.name !== params.id);
		return ok(null);
	}),

	http.post('/api/cron/:id/trigger', async ({ params }) => {
		await delay(LATENCY);
		const idx = cronJobs.findIndex((j) => j.id === params.id || j.name === params.id);
		if (idx === -1) return err('Cron job not found', 404);
		// Simulate a completed run being added to history
		const jobId = cronJobs[idx].id;
		const runId = `run_${Date.now()}`;
		const startedAt = new Date().toISOString();
		const run: CronJobRun = {
			id: runId,
			cron_job_id: jobId,
			status: 'success',
			exit_code: 0,
			output: '[mock] Manual trigger completed successfully',
			started_at: startedAt,
			finished_at: new Date(Date.now() + 1200).toISOString(),
			attempt: 1
		};
		if (!cronRuns[jobId]) cronRuns[jobId] = [];
		cronRuns[jobId] = [run, ...cronRuns[jobId]];
		cronJobs[idx] = {
			...cronJobs[idx],
			last_run_at: startedAt,
			last_run_status: 'success',
			updated_at: new Date().toISOString()
		};
		return ok({ message: 'Cron job triggered', cron_job_id: jobId });
	}),

	http.get('/api/cron/:id/runs', async ({ params, request }) => {
		await delay(LATENCY);
		const job = cronJobs.find((j) => j.id === params.id || j.name === params.id);
		if (!job) return err('Cron job not found', 404);
		const url = new URL(request.url);
		const limit = Math.min(parseInt(url.searchParams.get('limit') ?? '50', 10), 200);
		const offset = parseInt(url.searchParams.get('offset') ?? '0', 10);
		const allRuns = cronRuns[job.id] ?? [];
		return ok(allRuns.slice(offset, offset + limit));
	}),

	// ── WebSocket: Cron run stream ────────────────────────────────────────────
	//
	// Simulates /api/cron/{id}/runs/stream.
	// On connect: sends a snapshot of the most recent run for the job (or null).
	// Every 8 s: emits a new simulated completed run frame.

	cronRunStream.addEventListener('connection', ({ client }) => {
		// Extract job id from the URL path: /api/cron/<id>/runs/stream
		const match = client.url.pathname.match(/\/api\/cron\/([^/]+)\/runs\/stream/);
		const jobId = match?.[1] ?? '';

		const existingRuns = cronRuns[jobId] ?? [];
		const latestRun = existingRuns[0] ?? null;

		// Send snapshot
		client.send(JSON.stringify({ type: 'snapshot', run: latestRun }));

		// Every 8 s emit a new mock run
		let runCounter = 0;
		const timer = setInterval(() => {
			runCounter += 1;
			const success = runCounter % 3 !== 0; // fail every 3rd
			const startedAt = new Date().toISOString();
			const finishedAt = new Date(Date.now() + 1_200).toISOString();
			const newRun = {
				id: `mock_run_${jobId}_${Date.now()}`,
				cron_job_id: jobId,
				status: success ? 'success' : 'failed',
				exit_code: success ? 0 : 1,
				output: success
					? `[mock] run #${runCounter} completed ok`
					: `[mock] run #${runCounter} exited with error`,
				started_at: startedAt,
				finished_at: finishedAt,
				attempt: 1
			};
			client.send(JSON.stringify({ type: 'run', run: newRun }));
		}, 8_000);

		client.addEventListener('close', () => clearInterval(timer));
	}),

	// ── WebSocket: live deploy view (see the mock deploy engine above) ─────────
	deploymentStreamHandler,

	// ── WebSocket: App log stream ─────────────────────────────────────────────
	//
	// Simulates /api/apps/{id}/logs/stream.
	// On connect: sends a snapshot of 5 seed lines.
	// Every 2 s: sends one incremental container_log line.

	appLogStream.addEventListener('connection', ({ client }) => {
		// Send initial snapshot
		client.send(
			JSON.stringify({
				type: 'snapshot',
				deploy_logs: [
					{
						level: 'info',
						trigger_source: 'manual',
						message: '[mock] Build started',
						ts: new Date().toISOString()
					},
					{
						level: 'info',
						trigger_source: 'manual',
						message: '[mock] Installing dependencies…',
						ts: new Date().toISOString()
					},
					{
						level: 'info',
						trigger_source: 'manual',
						message: '[mock] Build complete',
						ts: new Date().toISOString()
					}
				],
				container_logs: [
					{ ts: new Date().toISOString(), line: '[mock] Server listening on :8080' },
					{ ts: new Date().toISOString(), line: '[mock] GET /healthz 200 OK' }
				]
			})
		);

		// Stream incremental lines every 2 s
		let counter = 0;
		const timer = setInterval(() => {
			counter += 1;
			client.send(
				JSON.stringify({
					type: 'container',
					line: `[mock] request #${counter} — ${new Date().toLocaleTimeString()}`
				})
			);
		}, 2000);

		client.addEventListener('close', () => clearInterval(timer));
	}),

	// ── WebSocket: System log stream ──────────────────────────────────────────
	//
	// Simulates /api/system/logs/stream.
	// On connect: sends a snapshot of recent deploy logs across all mock apps.
	// Every 3 s: sends one incremental deploy or container log line.

	systemLogStream.addEventListener('connection', ({ client }) => {
		const mockAppNames = apps.slice(0, 3).map((a) => ({ id: a.id, name: a.name }));

		// Send initial snapshot
		client.send(
			JSON.stringify({
				type: 'snapshot',
				deploy_logs: mockAppNames.flatMap((app) => [
					{
						app_id: app.id,
						app_name: app.name,
						level: 'info',
						trigger_source: 'manual',
						message: `[mock] ${app.name} — server started`,
						ts: new Date().toISOString()
					}
				])
			})
		);

		// Stream incremental lines every 3 s, cycling through mock apps
		let counter = 0;
		const timer = setInterval(() => {
			counter += 1;
			const app = mockAppNames[counter % mockAppNames.length];
			if (!app) return;
			const isContainer = counter % 3 !== 0;
			if (isContainer) {
				client.send(
					JSON.stringify({
						type: 'container',
						app_id: app.id,
						app_name: app.name,
						line: `[mock] ${app.name} — GET / 200 OK (#${counter})`
					})
				);
			} else {
				client.send(
					JSON.stringify({
						type: 'deploy',
						app_id: app.id,
						app_name: app.name,
						level: 'info',
						message: `[mock] ${app.name} — health check passed`,
						ts: new Date().toISOString()
					})
				);
			}
		}, 3000);

		client.addEventListener('close', () => clearInterval(timer));
	}),

	// ── GitHub Apps (Phase 4.4) ────────────────────────────────────────────

	http.get('/api/github-apps', async () => {
		await delay(LATENCY);
		return ok(githubApps);
	}),

	http.post('/api/github-apps/complete', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { state?: string | null };
		if (!body.state) return err('missing_state', 400);
		return ok({ completed: true });
	}),

	http.post('/api/github-apps/manifest', async () => {
		await delay(LATENCY);
		// In mock mode: simulate the manifest flow — return a github_url that points
		// back to the integrations page (triggering the ?mock_github_connected=1 handler)
		// and a dummy manifest string (never actually POSTed to GitHub in dev).
		return ok({
			github_url: '/integrations/github?mock_github_connected=1',
			manifest: '{}'
		});
	}),

	http.post('/api/github-apps/:id/install', async ({ params }) => {
		await delay(LATENCY);
		const app = githubApps.find((a) => a.id === params.id);
		if (!app) return err('GitHub App not found', 404);
		// The real server returns a single-use, admin-bound state on the URL.
		return ok({
			url: `https://github.com/apps/${encodeURIComponent(app.slug)}/installations/new?state=${randomHex(32)}`
		});
	}),

	http.patch('/api/github-apps/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = githubApps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('GitHub App not found', 404);
		const body = (await request.json()) as { name?: string; installation_id?: number | null };
		const updated = { ...githubApps[idx], ...body, updated_at: new Date().toISOString() };
		// Recompute derived fields
		updated.installed = updated.installation_id != null;
		githubApps[idx] = updated;
		return ok(githubApps[idx]);
	}),

	http.get('/api/github-apps/:id/repos', async ({ params }) => {
		await delay(LATENCY);
		const repos = MOCK_GITHUB_REPOS[params.id as string] ?? [];
		return ok(repos);
	}),

	http.get('/api/github-apps/:id/repos/:owner/:repo/branches', async ({ params }) => {
		await delay(LATENCY);
		const key = `${params.owner}/${params.repo}`;
		const branches = MOCK_GITHUB_BRANCHES[key] ?? [{ name: 'main' }];
		return ok(branches);
	}),

	http.put('/api/apps/:id/github-app', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { github_app_id: string | null };
		apps[idx] = {
			...apps[idx],
			github_app_id: body.github_app_id,
			updated_at: new Date().toISOString()
		};
		return ok(apps[idx]);
	}),

	http.get('/api/apps/:id/github-prs', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		if (!app.github_app_id) return err('No GitHub App linked', 422);
		// Return mock open PRs for demonstration
		const mockPRs: GithubPR[] = [
			{
				number: 42,
				title: 'feat: add user authentication',
				head_sha: 'abc123def456',
				head_ref: 'feat/user-auth',
				pr_url: 'https://github.com/example/api-service/pull/42'
			},
			{
				number: 37,
				title: 'fix: resolve memory leak in worker',
				head_sha: 'def789abc012',
				head_ref: 'fix/memory-leak',
				pr_url: 'https://github.com/example/api-service/pull/37'
			},
			{
				number: 31,
				title: 'chore: update dependencies',
				head_sha: '789abc012def',
				head_ref: 'chore/update-deps',
				pr_url: 'https://github.com/example/api-service/pull/31'
			}
		];
		return ok(mockPRs);
	}),

	http.delete('/api/github-apps/:id', async ({ params }) => {
		await delay(LATENCY);
		githubApps = githubApps.filter((a) => a.id !== params.id);
		return ok(null);
	}),

	// ── Notification Integrations ──────────────────────────────────────────────

	http.get('/api/integrations', async () => {
		await delay(LATENCY);
		return ok(integrations);
	}),

	http.post('/api/integrations', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			kind: string;
			config: Record<string, unknown>;
			quiet_hours?: Integration['quiet_hours'];
		};
		const newId = `int_${Date.now()}`;
		const stored = storeIntegrationConfig(newId, body.config ?? {});
		const needed: Record<string, string> = {
			telegram: 'bot_token',
			discord: 'webhook_url',
			slack: 'webhook_url',
			email: 'smtp_password',
			ntfy: 'topic',
			pushover: 'app_token',
			gotify: 'app_token',
			webhook: 'webhook_url'
		};
		if (!stored.saved_secrets.includes(needed[body.kind])) {
			return err(`invalid config for ${body.kind}: missing field \`${needed[body.kind]}\``, 400);
		}
		const newIntegration: Integration = {
			id: newId,
			name: body.name,
			kind: body.kind as Integration['kind'],
			...stored,
			enabled: true,
			quiet_hours: body.quiet_hours ?? { mode: 'global' },
			last_sent_at: null,
			last_error: null,
			last_error_at: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		integrations = [...integrations, newIntegration];
		return HttpResponse.json({ success: true, data: newIntegration, error: null }, { status: 201 });
	}),

	http.get('/api/integrations/:id', async ({ params }) => {
		await delay(LATENCY);
		const integration = integrations.find((i) => i.id === params.id);
		if (!integration) return err('Integration not found', 404);
		return ok(integration);
	}),

	http.patch('/api/integrations/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = integrations.findIndex((i) => i.id === params.id);
		if (idx === -1) return err('Integration not found', 404);
		const { config, ...body } = (await request.json()) as Partial<
			Pick<Integration, 'name' | 'enabled' | 'quiet_hours'> & { config: Record<string, unknown> }
		>;
		integrations[idx] = {
			...integrations[idx],
			...body,
			...(config && storeIntegrationConfig(integrations[idx].id, config)),
			updated_at: new Date().toISOString()
		};
		return ok(integrations[idx]);
	}),

	http.delete('/api/integrations/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = integrations.some((i) => i.id === params.id);
		if (!exists) return err('Integration not found', 404);
		integrations = integrations.filter((i) => i.id !== params.id);
		// Rule links go with the channel.
		alertRules = alertRules.map((r) => ({
			...r,
			integration_ids: r.integration_ids.filter((id) => id !== params.id)
		}));
		return ok({ deleted: params.id });
	}),

	http.post('/api/integrations/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const integration = integrations.find((i) => i.id === params.id);
		if (!integration) return err('Integration not found', 404);
		if (!integration.enabled) return err('Integration is disabled', 400);
		if (integration.last_error) {
			integration.last_error_at = new Date().toISOString();
			return err(`The test didn't arrive: ${integration.last_error}`, 400);
		}
		integration.last_sent_at = new Date().toISOString();
		return ok({ message: 'Test notification sent' });
	}),

	// ── WebSocket: All-databases log stream ───────────────────────────────────
	//
	// Simulates /api/databases/logs/stream.
	// On connect: sends a snapshot of container log lines tagged with db_id/db_name.
	// Every 3 s: sends one incremental container log line cycling through mock dbs.

	allDatabaseLogStream.addEventListener('connection', ({ client }) => {
		const mockDbs = databases.slice(0, 3).map((d) => ({ id: d.id, name: d.name }));

		// Send initial snapshot
		client.send(
			JSON.stringify({
				type: 'snapshot',
				container_logs: mockDbs.flatMap((db) => [
					{
						db_id: db.id,
						db_name: db.name,
						ts: new Date().toISOString(),
						line: `[mock] ${db.name} — engine started`
					}
				])
			})
		);

		// Stream incremental lines every 3 s, cycling through mock databases
		let counter = 0;
		const timer = setInterval(() => {
			counter += 1;
			const db = mockDbs[counter % mockDbs.length];
			if (!db) return;
			client.send(
				JSON.stringify({
					type: 'container',
					db_id: db.id,
					db_name: db.name,
					ts: new Date().toISOString(),
					line: `[mock] ${db.name} — connection #${counter}`
				})
			);
		}, 3000);

		client.addEventListener('close', () => clearInterval(timer));
	}),

	terminalStream.addEventListener('connection', ({ client }) => {
		const url = new URL(client.url);
		const ticket = url.searchParams.get('ticket') ?? '';
		const grant = terminalTickets.get(ticket);
		terminalTickets.delete(ticket);
		if (!grant) {
			client.send(
				JSON.stringify({
					type: 'error',
					message: 'Terminal ticket is invalid, expired or already used'
				})
			);
			client.close();
			return;
		}
		const targetKind = grant.kind;
		const targetId = grant.id;
		const label =
			targetKind === 'server'
				? 'server'
				: targetKind === 'app'
					? (apps.find((app) => app.id === targetId)?.name ?? targetId)
					: (databases.find((db) => db.id === targetId)?.name ?? targetId);

		client.send(JSON.stringify({ type: 'ready' }));
		client.send(
			JSON.stringify({
				type: 'output',
				data: `Connected to ${label}\r\n$ `
			})
		);

		client.addEventListener('message', (event) => {
			const raw = typeof event.data === 'string' ? event.data : '';
			let frame: { type?: string; data?: string };
			try {
				frame = JSON.parse(raw);
			} catch {
				return;
			}

			if (frame.type !== 'input') return;
			const input = frame.data ?? '';
			const trimmed = input.replace(/\r/g, '').trim();
			if (!trimmed) {
				client.send(JSON.stringify({ type: 'output', data: '$ ' }));
				return;
			}

			if (trimmed === 'exit') {
				client.send(JSON.stringify({ type: 'output', data: 'logout\r\n' }));
				client.send(JSON.stringify({ type: 'exit', code: 0 }));
				client.close();
				return;
			}

			client.send(
				JSON.stringify({
					type: 'output',
					data: `${trimmed}\r\n[mock:${label}] ${trimmed}\r\n$ `
				})
			);
		});
	}),

	// ── WebSocket: Database log stream ────────────────────────────────────────
	//
	// Simulates /api/databases/{id}/logs/stream.
	// On connect: sends a snapshot of 3 seed container log lines.
	// Every 3 s: sends one incremental container log line.

	databaseLogStream.addEventListener('connection', ({ client }) => {
		// Send initial snapshot
		client.send(
			JSON.stringify({
				type: 'snapshot',
				container_logs: [
					{ ts: new Date().toISOString(), line: '[mock] Database engine started' },
					{ ts: new Date().toISOString(), line: '[mock] Listening on port 5432' },
					{ ts: new Date().toISOString(), line: '[mock] Ready to accept connections' }
				]
			})
		);

		// Stream incremental lines every 3 s
		let counter = 0;
		const timer = setInterval(() => {
			counter += 1;
			client.send(
				JSON.stringify({
					type: 'container',
					ts: new Date().toISOString(),
					line: `[mock] connection #${counter} — ${new Date().toLocaleTimeString()}`
				})
			);
		}, 3000);

		client.addEventListener('close', () => clearInterval(timer));
	}),

	// ── Storage Providers ────────────────────────────────────────────────────

	http.get('/api/storage-providers', async () => {
		await delay(LATENCY);
		return ok(storageProviders);
	}),

	http.post('/api/storage-providers', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as any;
		const now = new Date().toISOString();
		const provider: StorageProvider = {
			id: `sp_${Date.now()}`,
			name: body.name,
			kind: body.kind ?? 's3',
			config: {
				endpoint: body.config?.endpoint ?? null,
				region: body.config?.region ?? '',
				bucket: body.config?.bucket ?? '',
				prefix: body.config?.prefix ?? null,
				access_key_id: body.config?.access_key_id ?? ''
			},
			created_at: now,
			updated_at: now
		};
		storageProviders = [provider, ...storageProviders];
		return ok(provider);
	}),

	http.patch('/api/storage-providers/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = storageProviders.findIndex((p) => p.id === params.id);
		if (idx === -1) return err('Storage provider not found', 404);
		const body = (await request.json()) as any;
		const updated: StorageProvider = {
			...storageProviders[idx],
			name: body.name ?? storageProviders[idx].name,
			config: { ...storageProviders[idx].config, ...(body.config ?? {}) },
			updated_at: new Date().toISOString()
		};
		storageProviders = storageProviders.map((p) => (p.id === params.id ? updated : p));
		return ok(updated);
	}),

	http.delete('/api/storage-providers/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = storageProviders.some((p) => p.id === params.id);
		if (!exists) return err('Storage provider not found', 404);
		storageProviders = storageProviders.filter((p) => p.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/storage-providers/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const p = storageProviders.find((sp) => sp.id === params.id);
		if (!p) return err('Storage provider not found', 404);
		return ok({ ok: true, message: `Connected to ${p.config.bucket} successfully.` });
	}),

	// ── Snapshots ────────────────────────────────────────────────────────────

	http.get('/api/snapshots', async ({ request }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const app = url.searchParams.get('app');
		const trigger = url.searchParams.get('trigger');
		const scope = url.searchParams.get('scope');
		let result = snapshots;
		if (app) result = result.filter((s) => s.trigger_app === app);
		if (trigger) result = result.filter((s) => s.trigger === trigger);
		if (scope) result = result.filter((s) => s.scope === scope);
		return ok(result);
	}),

	http.post('/api/snapshots', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as any;
		const now = new Date().toISOString();
		const snap: Snapshot = {
			id: `snap_${Date.now()}`,
			name: body.name ?? `Manual ${new Date().toLocaleString()}`,
			status: 'completed',
			trigger: 'manual',
			scope: 'full',
			trigger_app: null,
			deployment_id: null,
			parent_snapshot_id: null,
			created_at: now,
			completed_at: now,
			delta_bytes: Math.floor(Math.random() * 5_000_000),
			total_bytes: 98_304_000,
			local_path: `/opt/tond/snapshots/snap_${Date.now()}`,
			remote_key: null,
			storage_provider_id: null,
			checksum_sha256: null,
			error_message: null,
			notes: null,
			remote_status: backupConfig.storage_provider_id ? 'uploading' : null,
			remote_error: null,
			archive_bytes: null
		};
		snapshots = [snap, ...snapshots];
		return ok(snap);
	}),

	http.delete('/api/snapshots/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = snapshots.some((s) => s.id === params.id);
		if (!exists) return err('Snapshot not found', 404);
		snapshots = snapshots.filter((s) => s.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/snapshots/:id/restore', async ({ params }) => {
		await delay(LATENCY * 2);
		const snap = snapshots.find((s) => s.id === params.id);
		if (!snap) return err('Snapshot not found', 404);
		const now = new Date().toISOString();
		const preRestore: Snapshot = {
			id: `snap_pre_${Date.now()}`,
			name: 'Pre-restore auto-snapshot',
			status: 'completed',
			trigger: 'pre_restore',
			scope: 'full',
			trigger_app: null,
			deployment_id: null,
			parent_snapshot_id: null,
			created_at: now,
			completed_at: now,
			delta_bytes: 1_000_000,
			total_bytes: 98_304_000,
			local_path: `/opt/tond/snapshots/snap_pre_${Date.now()}`,
			remote_key: null,
			storage_provider_id: null,
			checksum_sha256: null,
			error_message: null,
			notes: null,
			remote_status: backupConfig.storage_provider_id ? 'uploading' : null,
			remote_error: null,
			archive_bytes: null
		};
		snapshots = [preRestore, ...snapshots];
		return ok({
			pre_restore_snapshot_id: preRestore.id,
			message: `Restore initiated. Pre-restore snapshot created: ${preRestore.id}`
		});
	}),

	http.post('/api/snapshots/:id/restore-app', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const snap = snapshots.find((s) => s.id === params.id);
		if (!snap) return err('Snapshot not found', 404);
		const body = (await request.json()) as any;
		return ok({ message: `App '${body.app_name}' rolled back successfully.` });
	}),

	http.post('/api/snapshots/:id/restore-database', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const snap = snapshots.find((s) => s.id === params.id);
		if (!snap) return err('Snapshot not found', 404);
		const body = (await request.json()) as { db_name: string };
		if (snap.scope !== 'full' && snap.scope !== `database:${body.db_name}`) {
			return err(
				`Database restore failed: This snapshot doesn't include the database '${body.db_name}'`,
				500
			);
		}
		await delay(LATENCY * 3);
		const safetyNet = mockDatabaseSnapshot(body.db_name, 'pre_restore');
		snapshots = [safetyNet, ...snapshots];
		return ok({
			message: `Database '${body.db_name}' restored. The data it held just before is saved as a backup.`,
			pre_restore_snapshot_id: safetyNet.id
		});
	}),

	http.get('/api/snapshots/config', async () => {
		await delay(LATENCY);
		return ok(backupConfig);
	}),

	http.put('/api/snapshots/config', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as Partial<BackupConfig>;
		backupConfig = { ...backupConfig, ...body };
		return ok(backupConfig);
	})
];
