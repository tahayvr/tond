/**
 * MSW request handlers for all /api/* endpoints.
 * Every handler returns the ApiResponse<T> envelope the real server uses:
 *   { success: true, data: T }  |  { success: false, data: null, error: { code, message, hint? } }
 */

import { http, HttpResponse, ws, delay } from 'msw';

// WebSocket links must be declared at module scope so MSW can track clients
// across the service worker boundary via a stable BroadcastChannel. Use paths
// relative to the page origin: a wildcard host ("ws://*/…") never matches.
const appLogStream = ws.link('/api/v1/apps/:id/logs/stream');
const systemLogStream = ws.link('/api/v1/system/logs/stream');
const allDatabaseLogStream = ws.link('/api/v1/databases/logs/stream');
const databaseLogStream = ws.link('/api/v1/databases/:id/logs/stream');
const cronRunStream = ws.link('/api/v1/cron/:id/runs/stream');
const terminalStream = ws.link('/api/v1/terminal/ws');
const deploymentStream = ws.link('/api/v1/apps/:id/deployments/:deploymentId/stream');

import {
	MOCK_USER,
	MOCK_USERS,
	MOCK_APPS,
	MOCK_PROJECTS,
	MOCK_TEMPLATES,
	MOCK_TEMPLATE_MEMORY,
	MOCK_SERVER_ARCH,
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
	MOCK_REMOTE_SNAPSHOTS,
	MOCK_APP_ENV_VARS,
	MOCK_PREVIEW_ENV_VARS,
	MOCK_CERTIFICATES,
	MOCK_APP_DOMAINS,
	MOCK_DNS_PROVIDERS,
	MOCK_DATABASE_LINKS,
	makeMockSystemMetrics,
	makeMockSystemSelf,
	makeMockMetrics,
	makeMockRequestMetrics,
	makeMockTrafficOverview,
	makeMockAccessLog,
	MOCK_DATABASE_SECRETS,
	MOCK_DB_CATALOG
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
	AppDomain,
	Deployment,
	DeploymentLogLine,
	DnsProvider,
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
	RemoteSnapshot,
	EnvVarEntry,
	DatabaseCredentials,
	DatabaseBackupStatus,
	DatabaseExtension,
	DbUpgradeState,
	GiteaToken,
	GitLabToken
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
import { updateHandlers } from './updates';
import {
	MOCK_COMPOSE_APP,
	MOCK_TEMPLATE_APP,
	MOCK_TEMPLATE_DOMAIN_ENV,
	MOCK_ADMIN_TEMPLATE_APP,
	MOCK_COMPOSE_DEPLOYMENTS,
	MOCK_COMPOSE_SERVICES,
	mockComposeSpecs,
	mockReleaseServices
} from './compose-fixtures';
import type { AppEgress, ComposeService, DeploymentService, UpdateEgressRequest } from '$lib/api';
import { errorBody, okBody } from './envelope';
import { MOCK_OPENAPI } from './openapi';

// Simulated network latency (ms). Set to 0 for instant responses.
const LATENCY = 200;

function ok<T>(data: T) {
	return HttpResponse.json(okBody(data));
}

/** 24 random letters and digits, like the server's generated passwords. */
function mockGeneratedSecret(): string {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
	const bytes = crypto.getRandomValues(new Uint8Array(24));
	return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join('');
}

function err(message: string, status = 400, code?: string) {
	return HttpResponse.json(errorBody(message, status, code), { status });
}

/** INVALID_INPUT about one request field. */
function fieldErr(message: string, field: string) {
	return HttpResponse.json(errorBody(message, 400, 'INVALID_INPUT', { field }), { status: 400 });
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
	const url = isRedisLike
		? `redis://${db.db_user}:${password}@tond.example.com:${db.host_port}`
		: `${db.db_type}://${db.db_user}:${password}@tond.example.com:${db.host_port}/${db.db_name}`;
	if (!db.tls) return url;
	if (db.db_type === 'postgres') return `${url}?sslmode=require`;
	if (db.db_type === 'mysql' || db.db_type === 'mariadb') return `${url}?ssl-mode=REQUIRED`;
	return url;
}

/** The catalog entry of a database's engine and variant. */
function mockCatalogEntry(dbType: string, variant: string) {
	return MOCK_DB_CATALOG.find((e) => e.db_type === dbType && e.variant === variant);
}

function mockVersionKey(v: string): number[] {
	return v === 'latest' ? [Infinity] : v.split('.').map(Number);
}

function mockNewer(a: string, b: string): boolean {
	const ka = mockVersionKey(a);
	const kb = mockVersionKey(b);
	for (let i = 0; i < Math.max(ka.length, kb.length); i++) {
		const x = ka[i] ?? 0;
		const y = kb[i] ?? 0;
		if (x !== y) return x > y;
	}
	return false;
}

function mockImage(dbType: string, variant: string, version: string): string {
	if (variant === 'pgvector') return `pgvector/pgvector:0.8.1-pg${version}`;
	if (variant === 'postgis') return `postgis/postgis:${version}-3.5-alpine`;
	const images: Record<string, string> = {
		postgres: `postgres:${version}-alpine`,
		mysql: `mysql:${version}`,
		mariadb: `mariadb:${version}`,
		redis: `redis:${version}-alpine`,
		keydb: 'eqalpha/keydb:latest',
		mongodb: `mongo:${version}`
	};
	return images[dbType] ?? `${dbType}:${version}`;
}

// Mirrors tond-server's domains.rs: previews need a preview domain on a different
// registrable domain from the dashboard. The mock approximates the Public Suffix
// List check by comparing the last two labels.
const PREVIEW_DOMAIN_REQUIRED =
	'Previews need a preview domain. Set one in Settings: a separate domain you own (e.g. example-previews.com, not a subdomain of the dashboard domain) with a wildcard DNS record *.<preview-domain> pointing at this server. Previews run pull-request code, so they must not share a site with the dashboard.';

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
		challenge: null,
		serves: { kind: 'unused' }
	});
}

const appDomains: Record<string, AppDomain[]> = structuredClone(MOCK_APP_DOMAINS);
let dnsProviders: DnsProvider[] = structuredClone(MOCK_DNS_PROVIDERS);

/** Server order: primary, aliases, redirects. */
function sortedDomains(appId: string): AppDomain[] {
	const rank = { primary: 0, alias: 1, redirect: 2 } as const;
	return [...(appDomains[appId] ?? [])].sort(
		(a, b) => rank[a.kind] - rank[b.kind] || a.created_at.localeCompare(b.created_at)
	);
}

/** Keep `App.domain` equal to the primary domain (the server's triggers). */
function syncPrimary(appId: string) {
	const idx = apps.findIndex((a) => a.id === appId);
	if (idx === -1) return;
	const primary = (appDomains[appId] ?? []).find((d) => d.kind === 'primary')?.domain ?? null;
	apps[idx] = { ...apps[idx], domain: primary };
}

/** Oldest alias becomes primary when the app has none. */
function promoteIfNeeded(appId: string) {
	const list = appDomains[appId] ?? [];
	if (list.some((d) => d.kind === 'primary')) return;
	const alias = [...list]
		.filter((d) => d.kind === 'alias')
		.sort((a, b) => a.created_at.localeCompare(b.created_at))[0];
	if (alias) alias.kind = 'primary';
}

/** The server's domain rules (app_domains.rs), or an error message. */
function checkMockDomain(appId: string, raw: string): string | null {
	const domain = normalizeDomain(raw);
	if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(domain)) {
		return `'${domain}' is not a valid domain name (expected something like app.example.com)`;
	}
	if (domain === MOCK_SETTINGS.dashboard_domain) {
		return `${domain} is the dashboard domain; choose another hostname for the app`;
	}
	const preview = MOCK_SETTINGS.preview_domain;
	if (preview && (domain === preview || domain.endsWith(`.${preview}`))) {
		return `${domain} is under the preview domain (${preview}), which only serves previews; choose a hostname outside it`;
	}
	for (const [otherId, list] of Object.entries(appDomains)) {
		if (otherId !== appId && list.some((d) => d.domain === domain)) {
			const other = apps.find((a) => a.id === otherId)?.name ?? otherId;
			return `${domain} is already used by the app '${other}'. Remove it there first.`;
		}
	}
	return null;
}

function mockDomainRow(
	domain: string,
	kind: AppDomain['kind'],
	redirectTo: string | null = null,
	code = 308
): AppDomain {
	return {
		domain,
		kind,
		redirect_to: redirectTo,
		redirect_code: code,
		service: null,
		port: null,
		created_at: new Date().toISOString(),
		automatic: !!MOCK_SETTINGS.apps_domain && domain.endsWith(`.${MOCK_SETTINGS.apps_domain}`),
		warning: mockDomainWarning(domain)
	};
}

// In-memory mutable state (mutations survive page navigation within the session)
// Starts as true so that `pnpm dev:setup` begins signed out with setup open.
// Like the real server, /api/v1/auth/status doesn't reveal it: open the setup link
// /setup?token=mock-setup-token (MOCK_SETUP_TOKEN) to reach the form.
// Flips to false once POST /api/v1/auth/setup succeeds.
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
/** Egress policies set through PUT /apps/:id/egress (apps start open). */
const egressByApp = new Map<string, AppEgress>();

function egressFor(appId: string): AppEgress {
	return (
		egressByApp.get(appId) ?? {
			policy: 'open',
			allowlist: [],
			preview_policy: 'none',
			firewall_available: true,
			warning: null,
			preview_warning: null,
			hosts: []
		}
	);
}

const terminalTickets = new Map<string, { kind: string; id: string }>();
let apps: App[] = structuredClone([
	...MOCK_APPS,
	MOCK_COMPOSE_APP,
	MOCK_TEMPLATE_APP,
	MOCK_ADMIN_TEMPLATE_APP
]);
const templateDomainEnv = structuredClone(MOCK_TEMPLATE_DOMAIN_ENV);

/** Mock of the server's template_version_newer: dotted numbers compare numerically. */
function versionNewer(candidate: string, current: string): boolean {
	const a = candidate.replace(/^v/i, '').split(/[.\-+]/);
	const b = current.replace(/^v/i, '').split(/[.\-+]/);
	for (let i = 0; i < Math.min(a.length, b.length); i++) {
		const [x, y] = [Number(a[i]), Number(b[i])];
		const cmp = Number.isNaN(x) || Number.isNaN(y) ? a[i].localeCompare(b[i]) : x - y;
		if (cmp !== 0) return cmp > 0;
	}
	return a.length > b.length;
}

function templateStatus(app: App) {
	const template = templates.find((t) => t.id === app.template_id);
	const recorded = templateDomainEnv[app.id];
	const url = (host: string) => `https://${host}`;
	const domainEnv =
		recorded && app.domain && app.domain !== recorded.host
			? Object.entries(recorded.keys).map(([key, t]) => ({
					key,
					current: t.replace('{app_url}', url(recorded.host)).replace('{app_host}', recorded.host),
					proposed: t.replace('{app_url}', url(app.domain!)).replace('{app_host}', app.domain!)
				}))
			: [];
	const updateAvailable =
		!!template && !!app.template_version && versionNewer(template.version, app.template_version);
	return {
		template_id: app.template_id!,
		template_name: template?.name ?? null,
		version: app.template_version ?? null,
		latest_version: template?.version ?? null,
		update_available: updateAvailable,
		upgrade_notes: template?.upgrade_notes ?? null,
		upgrade_blocked: null,
		domain_env: domainEnv,
		domain_env_edited: []
	};
}
/** Services of compose apps (app id → services). */
const composeServices: Record<string, ComposeService[]> = structuredClone(MOCK_COMPOSE_SERVICES);
let projects: Project[] = structuredClone(MOCK_PROJECTS);
const templates: Template[] = structuredClone(MOCK_TEMPLATES);
let alertRules: AlertRule[] = structuredClone(MOCK_ALERT_RULES);
let activeAlerts: ActiveAlert[] = structuredClone(MOCK_ACTIVE_ALERTS);
const alertHistory: AlertEpisode[] = structuredClone(MOCK_ALERT_HISTORY);
let alertSettings: AlertSettings = structuredClone(MOCK_ALERT_SETTINGS);
const deployments: Record<string, Deployment[]> = structuredClone({
	...MOCK_DEPLOYMENTS,
	...MOCK_COMPOSE_DEPLOYMENTS
});
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
	const entry = mockCatalogEntry(db.db_type, db.variant);
	const upgrade_targets =
		db.custom_image || db.upgrade || !entry?.supports_upgrade
			? []
			: entry.versions.filter((v) => mockNewer(v, db.version));
	let access_warning: string | null = null;
	if (db.public_access && db.allowlist.length === 0) {
		access_warning =
			'Open to every address on the internet. Add the addresses that need to connect to the allowlist, or make the database private and use an SSH tunnel.';
	}
	return {
		...db,
		connection_string: mask(creds.connection_string),
		public_connection_string: mask(creds.public_connection_string),
		internal_connection_string: mask(creds.internal_connection_string),
		upgrade_targets,
		access_warning,
		allowlist_enforced: db.public_access && db.allowlist.length > 0,
		supports_tls: entry?.supports_tls ?? false,
		backup_warning:
			db.backup_schedule || backupConfig.enabled
				? null
				: `Nothing backs up ${db.name} automatically. Add a schedule, or turn on scheduled snapshots on the Snapshots page.`
	};
}

/** Next run of a (mock) cron expression: "m h * * *" daily, else in an hour. */
function mockNextRun(schedule: string): string {
	const [m, h] = schedule.split(/\s+/);
	const next = new Date();
	if (/^\d+$/.test(m) && /^\d+$/.test(h)) {
		next.setUTCHours(Number(h), Number(m), 0, 0);
		if (next.getTime() <= Date.now()) next.setUTCDate(next.getUTCDate() + 1);
	} else {
		next.setTime(Date.now() + 3600_000);
	}
	return next.toISOString();
}

function mockBackupStatus(db: ManagedDatabase): DatabaseBackupStatus {
	const own = snapshots.filter((s) => s.scope === `database:${db.name}`);
	const held = snapshots.filter(
		(s) => s.status === 'completed' && (s.scope === 'full' || s.scope === `database:${db.name}`)
	);
	const last = own.find((s) => s.status === 'completed' || s.status === 'failed');
	return {
		schedule: db.backup_schedule,
		retention: db.backup_retention,
		next_run_at: db.backup_schedule ? mockNextRun(db.backup_schedule) : null,
		full_snapshots_enabled: backupConfig.enabled,
		full_schedule: backupConfig.enabled ? backupConfig.full_schedule_cron : null,
		last_backup_at:
			held
				.map((s) => s.created_at)
				.sort()
				.at(-1) ?? null,
		last_status: (last?.status as 'completed' | 'failed' | undefined) ?? null,
		last_error: last?.error_message ?? null,
		warning:
			db.backup_schedule || backupConfig.enabled
				? null
				: `Nothing backs up ${db.name} automatically. Add a schedule, or turn on scheduled snapshots on the Snapshots page.`
	};
}

/** Installed extensions per database (mock state). */
const dbExtensions: Record<string, Record<string, string>> = {
	db_pg1: { vector: '0.8.1', pg_trgm: '1.6' }
};

/** Walk a mock upgrade through its steps, then wait for confirmation. */
function runMockUpgrade(id: string) {
	const steps = [
		'Disconnecting apps',
		'Backing up the current data',
		'Starting the new version next to the current one',
		'Copying the data into the new version',
		'Switching to the new version'
	];
	steps.forEach((step, i) => {
		setTimeout(
			() => {
				const idx = databases.findIndex((d) => d.id === id);
				const u = databases[idx]?.upgrade;
				if (!u || u.status !== 'running') return;
				databases[idx] = { ...databases[idx], upgrade: { ...u, step } };
			},
			700 * (i + 1)
		);
	});
	setTimeout(
		() => {
			const idx = databases.findIndex((d) => d.id === id);
			const db = databases[idx];
			const u = db?.upgrade;
			if (!u || u.status !== 'running') return;
			databases[idx] = {
				...db,
				version: u.to_version,
				image_used: u.to_image,
				upgrade: {
					...u,
					status: 'awaiting_confirmation',
					step: `Running version ${u.to_version}. Check your apps, then confirm or roll back.`,
					finished_at: new Date().toISOString(),
					rollback_dir: `/opt/tond/data/db-upgrades/${db.name}/previous`
				}
			};
		},
		700 * (steps.length + 1)
	);
}

/** Mock Git tokens (the token value is never returned). */
let gitlabTokens: GitLabToken[] = [
	{
		id: 'gl_1',
		name: 'Work GitLab',
		gitlab_url: 'https://gitlab.com',
		username: 'dev',
		created_at: '2025-03-01T10:00:00.000Z'
	}
];
let giteaTokens: GiteaToken[] = [
	{
		id: 'gt_1',
		name: 'Codeberg',
		base_url: 'https://codeberg.org',
		username: 'dev',
		created_at: '2025-03-02T10:00:00.000Z'
	}
];

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
		github: `/api/v1/webhooks/github/${app.id}`,
		gitlab: `/api/v1/webhooks/gitlab/${app.id}`,
		gitea: `/api/v1/webhooks/gitea/${app.id}`,
		generic: `/api/v1/webhooks/generic/${app.id}`
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
// Only the first provider's bucket holds snapshots in the mock.
let remoteSnapshots: RemoteSnapshot[] = structuredClone(MOCK_REMOTE_SNAPSHOTS);
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
		return 'nothing to change: send at least one variable to set or unset';
	}
	const bad = [...set, ...unset].find((key) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key));
	if (bad) return `'${bad}' is not a valid variable name`;
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
//   rate-limited     POST /api/v1/auth/login gets 429 with Retry-After: 30
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

const CREDENTIAL_PATHS = ['/api/v1/auth/'];

// ── Mock deploy engine ────────────────────────────────────────────────────────
//
// POST /api/v1/apps/:id/deploy (and rollback) start a scripted deployment that
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
		compose: [
			null,
			'Build and pull service images',
			'Start services',
			'Wait for services to be ready',
			'Switch traffic'
		],
		'compose-rollback': [
			null,
			"Reuse the deployment's images",
			'Start services',
			'Wait for services to be ready',
			'Switch traffic'
		],
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
	const isCompose = app.source_type === 'compose';
	const plan = opts.rollbackOf ? (isCompose ? 'compose-rollback' : 'rollback') : app.source_type;
	const release = id.slice(4, 16).padEnd(12, '0');
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
		rollback_unavailable_reason: null,
		...(isCompose
			? {
					services: opts.rollbackOf?.services
						? opts.rollbackOf.services.map((s) => ({ ...s, source: 'reuse', status: 'pending' }))
						: mockReleaseServices(release, 'pending')
				}
			: {})
	};
	deployments[app.id] = [deployment, ...(deployments[app.id] ?? [])];
	deploymentLogs[id] = [];
	const appIdx = apps.findIndex((a) => a.id === app.id);
	// Like the server: a serving app keeps its status (and its route) while it
	// redeploys; only an app with nothing running shows 'building'.
	const serving = app.status === 'running' || app.status === 'unhealthy';
	if (appIdx !== -1 && !serving) apps[appIdx] = { ...apps[appIdx], status: 'building' };

	const fail = app.name === 'worker-queue';
	const tag = `tond-${app.name}:${release}`;
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
			if (appIdx !== -1 && !serving) apps[appIdx] = { ...apps[appIdx], status: 'failed' };
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
	} else if (isCompose) {
		const setServices = (status: (s: DeploymentService) => string) =>
			updateDeployment(app.id, id, (d) => ({
				...d,
				services: (d.services ?? []).map((s) => ({ ...s, status: status(s) }))
			}));
		script.push([400, () => phase('build')]);
		if (!opts.rollbackOf) {
			script.push(
				[300, () => setServices((s) => (s.source === 'build' ? 'building' : 'pulling'))],
				[700, () => log('info', '[admin] Building image tond-shop-stack/admin:' + release)],
				[900, () => log('info', '[cache] Pulling redis:7-alpine')],
				[800, () => setServices((s) => (s.source === 'pull' ? 'ready' : s.status))],
				[1200, () => log('info', '[admin] #8 exporting to image')],
				[400, () => setServices(() => 'ready')]
			);
		} else {
			script.push([300, () => setServices(() => 'ready')]);
		}
		script.push(
			[500, () => phase('start')],
			[200, () => setServices(() => 'starting')],
			[300, () => log('info', 'Starting services with docker compose up...')],
			[800, () => phase('health')],
			[
				900,
				() =>
					setServices((s) => (s.service === 'cache' || s.service === 'db' ? 'healthy' : s.status))
			],
			[900, () => setServices(() => 'healthy')],
			[300, () => log('info', '[web] Ready')],
			[300, () => phase('release')],
			[500, () => setServices(() => 'running')],
			[300, finish]
		);
		let waitC = 0;
		for (const [ms, action] of script) {
			waitC += ms;
			setTimeout(action, waitC);
		}
		return deployment;
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
	const match = client.url.pathname.match(/\/api\/v1\/apps\/([^/]+)\/deployments\/([^/]+)\/stream/);
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

/** Mock of GET /api/v1/domains/check (server: handlers/domains.rs). */
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
	const parent = domain.split('.').slice(1).join('.');
	const wildcard = parent.includes('.')
		? (certificates.find((c) => c.domain === `*.${parent}` && c.status === 'active') ?? null)
		: null;
	const own = certificates.find((c) => c.domain === domain) ?? null;
	const cert = own ?? wildcard;
	const provider =
		dnsProviders.find((p) => p.zones.some((z) => domain === z || domain.endsWith(`.${z}`))) ?? null;
	return {
		covered_by: own ? null : (wildcard?.domain ?? null),
		dns_provider: provider?.name ?? null,
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
					domain: cert.domain,
					cert_type: cert.cert_type,
					challenge: cert.challenge,
					issuer: cert.issuer,
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

/** Ports a few well-known images expose (mock of POST /api/v1/images/inspect). */
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
	// System → Updates (./updates.ts)
	...updateHandlers,

	// The OpenAPI document is plain JSON, not an envelope.
	http.get('/api/v1/openapi.json', () => HttpResponse.json(MOCK_OPENAPI)),

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
		if (scenario === 'rate-limited' && path === '/api/v1/auth/login') {
			return HttpResponse.json(errorBody('Rate limit exceeded. Try again later.', 429), {
				status: 429,
				headers: { 'Retry-After': '30' }
			});
		}
	}),

	// ── Auth ────────────────────────────────────────────────────────────────

	http.get('/api/v1/auth/status', async () => {
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

	http.post('/api/v1/auth/setup/check', async ({ request }) => {
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

	http.post('/api/v1/auth/login', async () => {
		await delay(LATENCY);
		if (mockScenario() === 'mfa') {
			return ok({ mfa_required: true });
		}
		mockSignedIn = true;
		return ok({ user: currentMockUser() });
	}),

	http.post('/api/v1/auth/logout', async () => {
		await delay(LATENCY);
		return ok(null);
	}),

	http.get('/api/v1/auth/me', async () => {
		await delay(LATENCY);
		return ok(currentMockUser());
	}),

	http.post('/api/v1/auth/setup', async ({ request }) => {
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

	http.get('/api/v1/users', async () => {
		await delay(LATENCY);
		return ok(users);
	}),

	http.post('/api/v1/users', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { email?: string; role?: string };
		const email = body.email?.trim() ?? '';
		if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
			return err('Enter a valid email address, like name@example.com.', 400);
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
			okBody({
				user: newUser,
				token: MOCK_PASSWORD_LINK,
				path: `/set-password?token=${MOCK_PASSWORD_LINK}`,
				expires_at: new Date(Date.now() + 86400_000).toISOString()
			}),
			{ status: 201 }
		);
	}),

	http.patch('/api/v1/users/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { role?: string; disabled?: boolean };
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('user', 404);
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

	http.delete('/api/v1/users/:id', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('user', 404);
		if (target.is_you)
			return err("Invalid input: You can't delete your own account. Ask another admin.");
		users = users.filter((u) => u.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/v1/users/:id/password-link', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('user', 404);
		return HttpResponse.json(
			{
				success: true,
				data: {
					user: target,
					token: MOCK_PASSWORD_LINK,
					path: `/set-password?token=${MOCK_PASSWORD_LINK}`,
					expires_at: new Date(Date.now() + 86400_000).toISOString()
				}
			},
			{ status: 201 }
		);
	}),

	http.get('/api/v1/users/:id/sessions', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('user', 404);
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

	http.delete('/api/v1/users/:id/sessions', async ({ params }) => {
		await delay(LATENCY);
		const target = users.find((u) => u.id === params.id);
		if (!target) return err('user', 404);
		const revoked = target.is_you ? target.active_sessions - 1 : target.active_sessions;
		users = users.map((u) =>
			u.id === params.id ? { ...u, active_sessions: u.active_sessions - revoked } : u
		);
		return ok({ revoked });
	}),

	http.delete('/api/v1/users/:id/sessions/:sessionId', async ({ params }) => {
		await delay(LATENCY);
		users = users.map((u) =>
			u.id === params.id ? { ...u, active_sessions: Math.max(0, u.active_sessions - 1) } : u
		);
		return ok({ revoked: 1 });
	}),

	// ── Set-password links (public) ───────────────────────────────────────────

	http.post('/api/v1/auth/password-link/check', async ({ request }) => {
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

	http.post('/api/v1/auth/password-link/complete', async ({ request }) => {
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

	http.get('/api/v1/tokens/catalog', async () => {
		await delay(LATENCY);
		return ok(MOCK_TOKEN_CATALOG);
	}),

	http.get('/api/v1/tokens', async () => {
		await delay(LATENCY);
		return ok(apiTokens);
	}),

	http.post('/api/v1/tokens', async ({ request }) => {
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
			return err('Give the token a name, e.g. the machine or workflow that uses it.');
		}
		if (!body.scopes?.length) return err('Choose at least one scope or a preset.');
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
		return HttpResponse.json({ success: true, data: { token, secret } }, { status: 201 });
	}),

	http.delete('/api/v1/tokens/:id', async ({ params }) => {
		await delay(LATENCY);
		const token = apiTokens.find((t) => t.id === params.id);
		if (!token) return err('token', 404);
		const updated = { ...token, revoked_at: token.revoked_at ?? new Date().toISOString() };
		apiTokens = apiTokens.map((t) => (t.id === params.id ? updated : t));
		return ok(updated);
	}),

	http.get('/api/v1/terminal/targets', async () => {
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

	http.post('/api/v1/terminal/tickets', async ({ request }) => {
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
		return HttpResponse.json({ success: true, data: { ticket, expires_in: 30 } }, { status: 201 });
	}),

	http.get('/api/v1/settings/host-terminal', async () => {
		await delay(LATENCY);
		return ok({ enabled: mockHostTerminalEnabled() });
	}),

	http.put('/api/v1/settings/host-terminal', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { enabled?: boolean };
		return err(
			`The host terminal can only be switched on or off on the server itself: run \`sudo tond settings host-terminal ${body.enabled ? 'on' : 'off'}\`.`,
			403
		);
	}),

	// ── Phase 7.1 — Password Management ─────────────────────────────────────

	http.post('/api/v1/auth/change-password', async ({ request }) => {
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

	http.get('/api/v1/auth/sessions', async () => {
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

	http.delete('/api/v1/auth/sessions/:id', async () => {
		await delay(LATENCY);
		return ok({ message: 'Session revoked' });
	}),

	http.delete('/api/v1/auth/sessions', async () => {
		await delay(LATENCY);
		return ok({ message: 'Revoked 1 session(s)' });
	}),

	// ── Phase 7.1 — TOTP 2FA ────────────────────────────────────────────────

	http.post('/api/v1/auth/2fa/setup', async ({ request }) => {
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

	http.post('/api/v1/auth/2fa/confirm', async ({ request }) => {
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

	http.post('/api/v1/auth/2fa/disable', async ({ request }) => {
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

	http.post('/api/v1/auth/2fa/verify', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { code: string };
		if (body.code !== '123456') {
			return err('Invalid TOTP or backup code', 401);
		}
		mockSignedIn = true;
		return ok({ user: currentMockUser() });
	}),

	// ── Projects ──────────────────────────────────────────────────────────────

	http.get('/api/v1/projects', async () => {
		await delay(LATENCY);
		const withCounts = projects.map((p) => ({
			...p,
			active_preview_count: (projectPreviews[p.id] ?? []).filter(
				(pp) => pp.status !== 'expired' && pp.status !== 'failed'
			).length
		}));
		return ok(withCounts);
	}),

	http.get('/api/v1/templates', async () => {
		await delay(LATENCY);
		return ok({
			templates,
			memory: MOCK_TEMPLATE_MEMORY,
			apps_domain: MOCK_SETTINGS.apps_domain,
			architecture: MOCK_SERVER_ARCH
		});
	}),

	http.post('/api/v1/templates/:id/deploy', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const template = templates.find((t) => t.id === params.id);
		if (!template) return err(`No template named ${params.id}`, 404);
		const body = (await request.json()) as TemplateDeployRequest;
		const name = (body.name ?? '').trim().toLowerCase();
		if (!/^[a-z0-9](?:[a-z0-9-]{0,38}[a-z0-9])?$/.test(name)) {
			return HttpResponse.json(
				errorBody('App name must be 1–40 lowercase letters, digits and hyphens', 400, undefined, {
					field: 'name'
				}),
				{ status: 400 }
			);
		}
		if (!template.architectures.includes(MOCK_SERVER_ARCH)) {
			return err(
				`${template.name} isn't available on ARM64: its images are published for x86_64 only.`,
				409,
				'CONFLICT'
			);
		}
		if (apps.some((a) => a.name === name))
			return err(`App already exists: ${name}`, 409, 'APP_ALREADY_EXISTS');
		if (template.ram_mb > MOCK_TEMPLATE_MEMORY.available_mb && !body.allow_low_memory) {
			return err(
				`${template.name} needs about ${template.ram_mb} MB of memory and this server has ${MOCK_TEMPLATE_MEMORY.available_mb} MB free.`,
				409,
				'INSUFFICIENT_MEMORY'
			);
		}
		const credentials: { key: string; label: string; value: string }[] = [];
		for (const input of template.inputs) {
			const given = (body.inputs?.[input.key] ?? '').trim();
			const problem =
				!given && input.required
					? `${input.label} is required`
					: given && input.kind === 'email' && !given.includes('@')
						? `${input.label} must be an email address`
						: null;
			if (problem) {
				return HttpResponse.json(
					errorBody(problem, 400, undefined, { field: `inputs.${input.key}` }),
					{ status: 400 }
				);
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
			template_id: template.id,
			template_version: template.version,
			admin_setup:
				template.admin_setup === 'seeded'
					? 'done'
					: template.admin_setup === 'first_visitor'
						? 'open'
						: null,
			created_at: now,
			updated_at: now
		};
		apps = [...apps, newApp];
		const d = startMockDeploy(newApp, {
			trigger_source: 'manual',
			trigger_detail: MOCK_USER.email
		});
		return HttpResponse.json(
			okBody({
				app_id: newApp.id,
				app_name: name,
				domain,
				deployment_id: d.id,
				credentials,
				setup_notes: template.setup_notes
			}),
			{ status: 202 }
		);
	}),

	http.post('/api/v1/projects', async ({ request }) => {
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

	http.get('/api/v1/projects/:id', async ({ params }) => {
		await delay(LATENCY);
		const project = projects.find((p) => p.id === params.id || p.name === params.id);
		if (!project) return err('Project not found', 404);
		const memberApps = apps.filter((a) => a.project_id === project.id);
		return ok({ ...project, apps: memberApps });
	}),

	http.put('/api/v1/projects/:id', async ({ params, request }) => {
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

	http.delete('/api/v1/projects/:id', async ({ params }) => {
		await delay(LATENCY);
		const project = projects.find((p) => p.id === params.id || p.name === params.id);
		if (project) {
			// Un-assign member apps
			apps = apps.map((a) => (a.project_id === project.id ? { ...a, project_id: null } : a));
		}
		projects = projects.filter((p) => p.id !== params.id && p.name !== params.id);
		return ok(null);
	}),

	http.post('/api/v1/projects/:id/deploy', async ({ params }) => {
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

	// PUT /api/v1/apps/:id/project — assign / un-assign an app from a project
	http.put('/api/v1/apps/:id/project', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { project_id: string | null };
		apps[idx] = { ...apps[idx], project_id: body.project_id, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Apps ─────────────────────────────────────────────────────────────────

	http.get('/api/v1/apps', async () => {
		await delay(LATENCY);
		return ok(apps);
	}),

	http.post('/api/v1/apps', async ({ request }) => {
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
				'Background workers have no domain: they serve no traffic. Leave the domain empty.'
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
		if (newApp.source_type === 'compose') {
			const specs = mockComposeSpecs(body.compose_content ?? '');
			const port = newApp.container_port;
			const wanted = (body as { compose_service?: string }).compose_service;
			const web =
				specs.find((s) => s.name === wanted) ??
				specs.find((s) => s.ports.includes(port)) ??
				specs.find((s) => s.ports.length > 0);
			composeServices[newApp.id] = specs.map((spec) => ({
				...spec,
				published_port:
					spec === web ? (spec.ports.includes(port) ? port : (spec.ports[0] ?? port)) : null,
				host_port: null,
				redeploy_required: spec === web,
				domains: spec === web && newApp.domain ? [newApp.domain] : [],
				status: 'pending',
				health: null,
				exit_code: null,
				container_id: null,
				running_image: null,
				image_digest: null,
				cpu: null,
				mem: null
			}));
		}
		return ok(newApp);
	}),

	http.get('/api/v1/apps/:id', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		return ok({ ...app, domain_warning: mockDomainWarning(app.domain) });
	}),

	http.get('/api/v1/apps/:id/databases', async ({ params }) => {
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

	http.delete('/api/v1/apps/:id', async ({ params }) => {
		await delay(LATENCY);
		apps = apps.filter((a) => a.id !== params.id);
		databaseLinks = databaseLinks.filter((l) => l.app_id !== params.id);
		return ok(null);
	}),

	http.post('/api/v1/apps/:id/deploy', async ({ params }) => {
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
				}
			},
			{ status: 202 }
		);
	}),

	http.put('/api/v1/apps/:id/readiness', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { readiness: string; ready_after_secs?: number };
		if (!['http', 'tcp', 'none'].includes(body.readiness)) {
			return err('readiness must be one of: http, tcp, none');
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

	http.get('/api/v1/apps/:id/egress', async ({ params }) => {
		await delay(LATENCY);
		if (!apps.some((a) => a.id === params.id)) return err('App not found', 404);
		return ok(egressFor(String(params.id)));
	}),

	http.put('/api/v1/apps/:id/egress', async ({ params, request }) => {
		await delay(LATENCY);
		if (!apps.some((a) => a.id === params.id)) return err('App not found', 404);
		const body = (await request.json()) as UpdateEgressRequest;
		const policies = ['open', 'restricted', 'none'];
		if (!policies.includes(body.policy)) {
			return fieldErr(`'${body.policy}' isn't a policy; use open, restricted or none`, 'policy');
		}
		const current = egressFor(String(params.id));
		const allowlist = (body.allowlist ?? current.allowlist).map((e) => e.trim().toLowerCase());
		const bad = allowlist.find((e) => e.startsWith('*.') || e.includes(' '));
		if (bad) {
			return fieldErr(`'${bad}' isn't an IPv4 address, a CIDR range or a hostname`, 'allowlist');
		}
		const preview_policy = body.preview_policy ?? current.preview_policy;
		if (
			allowlist.filter(Boolean).length === 0 &&
			(body.policy === 'restricted' || preview_policy === 'restricted')
		) {
			return fieldErr(
				'A restricted policy needs at least one destination. Add the hosts or addresses the app calls, or choose "none" to block the internet entirely.',
				'allowlist'
			);
		}
		const list = allowlist.filter(Boolean);
		egressByApp.set(String(params.id), {
			...current,
			policy: body.policy,
			allowlist: list,
			preview_policy,
			hosts: list
				.filter((e) => /[a-z]/.test(e.split(':')[0]))
				.map((e) => ({ host: e.split(':')[0], addresses: ['192.0.2.10'] }))
		});
		return ok(egressFor(String(params.id)));
	}),

	http.put('/api/v1/apps/:id/port', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { container_port: number };
		const port = Number(body.container_port);
		if (!Number.isInteger(port) || port < 1 || port > 65535) {
			return err('container_port must be between 1 and 65535');
		}
		apps[idx] = { ...apps[idx], container_port: port, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Source inspection and domain checks ──

	http.post('/api/v1/git/branches', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as { url: string; github_app_id?: string };
		if (!/^https?:\/\//.test(body.url ?? '')) {
			return err('Branches can only be listed for http(s) Git URLs; type the branch name instead');
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

	http.post('/api/v1/images/inspect', async ({ request }) => {
		await delay(LATENCY * 4);
		const body = (await request.json()) as { image: string; registry_credential_id?: string };
		const image = (body.image ?? '').trim();
		if (!image || /\s/.test(image)) {
			return err('Enter an image reference like nginx:alpine or ghcr.io/org/app:1.2');
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

	http.get('/api/v1/domains/check', async ({ request }) => {
		await delay(LATENCY * 2);
		const domain = (new URL(request.url).searchParams.get('domain') ?? '').trim().toLowerCase();
		if (!/^([a-z0-9]([a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*$/.test(domain)) {
			return err(`'${domain}' isn't a valid domain name`);
		}
		return ok(mockDomainCheck(domain));
	}),

	http.post('/api/v1/apps/:id/stop', async ({ params }) => {
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

	http.post('/api/v1/apps/:id/start', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = { ...apps[idx], status: 'running', updated_at: new Date().toISOString() };
		return ok(null);
	}),

	http.post('/api/v1/apps/:id/admin-setup/dismiss', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id || a.name === params.id);
		if (idx === -1) return err('App not found', 404);
		const current = apps[idx];
		if (current.admin_setup === 'open' || current.admin_setup === 'failed') {
			apps[idx] = {
				...current,
				admin_setup: 'dismissed',
				admin_setup_error: null,
				updated_at: new Date().toISOString()
			};
		}
		return ok(apps[idx]);
	}),

	http.post('/api/v1/apps/:id/restart', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = { ...apps[idx], status: 'running', updated_at: new Date().toISOString() };
		return ok(null);
	}),

	// ── App environment (masked reads, per-key patches) ──

	http.get('/api/v1/apps/:id/env', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		return ok(appEnvResponse(app));
	}),

	http.get('/api/v1/apps/:id/env/:key', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const value = envStore(app.id)[params.key as string];
		if (value === undefined) return err(`variable '${params.key}'`, 404);
		return ok({ key: params.key, value });
	}),

	http.patch('/api/v1/apps/:id/env', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const body = (await request.json()) as { set?: Record<string, string>; unset?: string[] };
		const problem = validateEnvPatch(body);
		if (problem) return err(problem);
		appEnvVars[app.id] = applyEnvPatch(envStore(app.id), body);
		return ok(appEnvResponse(app));
	}),

	http.get('/api/v1/apps/:id/preview-env/:key', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const value = previewStore(app.id)[params.key as string];
		if (value === undefined) return err(`preview override '${params.key}'`, 404);
		return ok({ key: params.key, value });
	}),

	http.patch('/api/v1/apps/:id/preview-env', async ({ params, request }) => {
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

	http.put('/api/v1/apps/:id/domain', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { domain: string };
		const domain = normalizeDomain(body.domain);
		if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/.test(domain)) {
			return err(
				`'${domain}' is not a valid domain name (expected something like app.example.com)`
			);
		}
		if (domain === MOCK_SETTINGS.dashboard_domain) {
			return err(`${domain} is the dashboard domain; choose another hostname for the app`);
		}
		const preview = MOCK_SETTINGS.preview_domain;
		if (preview && (domain === preview || domain.endsWith(`.${preview}`))) {
			return err(
				`${domain} is under the preview domain (${preview}), which only serves previews; choose a hostname outside it`
			);
		}
		const list = (appDomains[apps[idx].id] ??= []);
		for (const d of list) if (d.kind === 'primary') d.kind = 'alias';
		const existing = list.find((d) => d.domain === domain);
		if (existing) Object.assign(existing, { kind: 'primary', redirect_to: null });
		else list.push(mockDomainRow(domain, 'primary'));
		syncPrimary(apps[idx].id);
		queueMockCertificate(domain);
		return ok({
			message: 'Domain updated, certificate provisioning queued',
			domain,
			warning: mockDomainWarning(domain)
		});
	}),

	// ── App domains ───────────────────────────────────────────────────────────

	http.get('/api/v1/apps/:id/domains', async ({ params }) => {
		await delay(LATENCY);
		if (!apps.some((a) => a.id === params.id)) return err('App not found', 404);
		return ok(sortedDomains(params.id as string));
	}),

	http.post('/api/v1/apps/:id/domains/www-pair', async ({ params, request }) => {
		await delay(LATENCY);
		const id = params.id as string;
		if (!apps.some((a) => a.id === id)) return err('App not found', 404);
		const body = (await request.json()) as {
			domain: string;
			canonical: 'apex' | 'www';
			primary?: boolean;
		};
		const input = normalizeDomain(body.domain);
		const apex = input.startsWith('www.') ? input.slice(4) : input;
		const www = `www.${apex}`;
		const [canonical, other] = body.canonical === 'apex' ? [apex, www] : [www, apex];
		const problem = checkMockDomain(id, canonical) ?? checkMockDomain(id, other);
		if (problem) return err(problem, problem.includes('already used') ? 409 : 400);
		const list = (appDomains[id] ??= []);
		const makePrimary = body.primary ?? true;
		if (makePrimary) for (const d of list) if (d.kind === 'primary') d.kind = 'alias';
		const upsert = (row: AppDomain) => {
			const i = list.findIndex((d) => d.domain === row.domain);
			if (i === -1) {
				list.push(row);
				queueMockCertificate(row.domain);
			} else list[i] = { ...list[i], ...row, created_at: list[i].created_at };
		};
		upsert(mockDomainRow(canonical, makePrimary ? 'primary' : 'alias'));
		for (const d of list) if (d.redirect_to === other) d.redirect_to = canonical;
		upsert(mockDomainRow(other, 'redirect', canonical));
		promoteIfNeeded(id);
		syncPrimary(id);
		return ok(sortedDomains(id));
	}),

	http.post('/api/v1/apps/:id/domains', async ({ params, request }) => {
		await delay(LATENCY);
		const id = params.id as string;
		if (!apps.some((a) => a.id === id)) return err('App not found', 404);
		const body = (await request.json()) as {
			domain: string;
			kind?: AppDomain['kind'];
			redirect_to?: string;
			redirect_code?: number;
		};
		const domain = normalizeDomain(body.domain);
		const problem = checkMockDomain(id, domain);
		if (problem) return err(problem, problem.includes('already used') ? 409 : 400);
		const list = (appDomains[id] ??= []);
		if (list.some((d) => d.domain === domain)) {
			return err(`${domain} is already a domain of this app`, 409);
		}
		const kind = body.kind ?? (list.some((d) => d.kind === 'primary') ? 'alias' : 'primary');
		if (kind === 'redirect') {
			const target = list.find((d) => d.domain === body.redirect_to);
			if (!target || target.kind === 'redirect') {
				return err(
					`${body.redirect_to ?? '(none)'} isn't a serving domain of this app. Redirects can only point at the app's own domains.`
				);
			}
		}
		if (kind === 'primary') for (const d of list) if (d.kind === 'primary') d.kind = 'alias';
		list.push(
			mockDomainRow(
				domain,
				kind,
				kind === 'redirect' ? (body.redirect_to ?? null) : null,
				body.redirect_code ?? 308
			)
		);
		promoteIfNeeded(id);
		syncPrimary(id);
		queueMockCertificate(domain);
		return ok(sortedDomains(id));
	}),

	http.patch('/api/v1/apps/:id/domains/:domain', async ({ params, request }) => {
		await delay(LATENCY);
		const id = params.id as string;
		const list = appDomains[id] ?? [];
		const row = list.find((d) => d.domain === params.domain);
		if (!row) return err(`${params.domain} is not a domain of this app`, 404);
		const body = (await request.json()) as {
			kind?: AppDomain['kind'];
			redirect_to?: string;
			redirect_code?: number;
		};
		const kind = body.kind ?? row.kind;
		if (row.kind === 'primary' && kind !== 'primary') {
			return err(`${row.domain} is the primary domain. Make another domain primary first.`, 409);
		}
		if (kind === 'redirect') {
			const pointing = list.filter((d) => d.redirect_to === row.domain).map((d) => d.domain);
			if (pointing.length) {
				return err(
					`${pointing.join(', ')} redirect${pointing.length === 1 ? 's' : ''} to ${row.domain}; point ${pointing.length === 1 ? 'it' : 'them'} at another domain first`,
					409
				);
			}
			const target = list.find((d) => d.domain === (body.redirect_to ?? row.redirect_to));
			if (!target || target.kind === 'redirect' || target.domain === row.domain) {
				return err(`choose a serving domain of this app to redirect to`);
			}
			row.redirect_to = target.domain;
			row.redirect_code = body.redirect_code ?? row.redirect_code;
		} else {
			row.redirect_to = null;
			row.redirect_code = 308;
		}
		if (kind === 'primary') for (const d of list) if (d.kind === 'primary') d.kind = 'alias';
		row.kind = kind;
		promoteIfNeeded(id);
		syncPrimary(id);
		return ok(sortedDomains(id));
	}),

	http.delete('/api/v1/apps/:id/domains/:domain', async ({ params }) => {
		await delay(LATENCY);
		const id = params.id as string;
		const list = appDomains[id] ?? [];
		const row = list.find((d) => d.domain === params.domain);
		if (!row) return err(`${params.domain} is not a domain of this app`, 404);
		const pointing = list.filter((d) => d.redirect_to === row.domain).map((d) => d.domain);
		if (pointing.length) {
			return err(
				`${pointing.join(', ')} redirect${pointing.length === 1 ? 's' : ''} to ${row.domain}. Remove ${pointing.length === 1 ? 'it' : 'them'} or point ${pointing.length === 1 ? 'it' : 'them'} at another domain first.`,
				409
			);
		}
		appDomains[id] = list.filter((d) => d !== row);
		promoteIfNeeded(id);
		syncPrimary(id);
		return ok(sortedDomains(id));
	}),

	// ── DNS providers ─────────────────────────────────────────────────────────

	http.get('/api/v1/dns-providers', async () => {
		await delay(LATENCY);
		return ok(dnsProviders);
	}),

	http.post('/api/v1/dns-providers', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as {
			name: string;
			kind: DnsProvider['kind'];
			zones: string[];
			token: string;
			config?: DnsProvider['config'];
		};
		if (!body.name?.trim()) return err('Give the provider a name');
		if (!body.token?.trim()) return err('Paste the API token');
		const zones = [...new Set(body.zones.map(normalizeDomain).filter(Boolean))];
		if (zones.length === 0) {
			return err('List at least one zone (the domain the provider hosts, e.g. example.com)');
		}
		if (body.kind === 'route53' && !body.config?.access_key_id?.trim()) {
			return err(
				'Route 53 needs the access key id (the secret access key goes in the token field)'
			);
		}
		const taken = dnsProviders.find((p) => p.zones.some((z) => zones.includes(z)));
		if (taken) {
			const zone = zones.find((z) => taken.zones.includes(z));
			return err(`${zone} is already handled by the DNS provider '${taken.name}'`, 409);
		}
		const now = new Date().toISOString();
		const provider: DnsProvider = {
			id: `dns_${Date.now()}`,
			name: body.name.trim(),
			kind: body.kind,
			zones,
			config: body.config ?? {},
			token_set: true,
			created_at: now,
			updated_at: now,
			last_used_at: null,
			last_error: null
		};
		dnsProviders = [...dnsProviders, provider];
		return ok(provider);
	}),

	http.patch('/api/v1/dns-providers/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = dnsProviders.findIndex((p) => p.id === params.id);
		if (idx === -1) return err(`DNS provider ${params.id}`, 404);
		const body = (await request.json()) as {
			name?: string;
			zones?: string[];
			token?: string;
			config?: DnsProvider['config'];
		};
		const current = dnsProviders[idx];
		dnsProviders[idx] = {
			...current,
			name: body.name?.trim() || current.name,
			zones: body.zones ? body.zones.map(normalizeDomain).filter(Boolean) : current.zones,
			config: body.config ?? current.config,
			updated_at: new Date().toISOString(),
			last_error: null
		};
		return ok(dnsProviders[idx]);
	}),

	http.delete('/api/v1/dns-providers/:id', async ({ params }) => {
		await delay(LATENCY);
		dnsProviders = dnsProviders.filter((p) => p.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/v1/dns-providers/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const provider = dnsProviders.find((p) => p.id === params.id);
		if (!provider) return err(`DNS provider ${params.id}`, 404);
		const labels: Record<DnsProvider['kind'], string> = {
			cloudflare: 'Cloudflare',
			hetzner: 'Hetzner DNS',
			digitalocean: 'DigitalOcean',
			route53: 'Amazon Route 53',
			challtestsrv: 'challtestsrv (test)'
		};
		const zones = provider.zones.map((zone) =>
			zone.endsWith('.invalid')
				? {
						zone,
						ok: false,
						message: `${labels[provider.kind]} answered 403: Unauthorized — check the token and its permissions`
					}
				: { zone, ok: true, message: `${labels[provider.kind]} can manage ${zone}` }
		);
		return ok({ ok: zones.every((z) => z.ok), zones });
	}),

	// ── Certificates ──────────────────────────────────────────────────────────

	http.post('/api/v1/certs/custom', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as { domain: string; cert_pem: string; key_pem: string };
		const domain = normalizeDomain(body.domain);
		if (!body.cert_pem.includes('-----BEGIN CERTIFICATE-----')) {
			return fieldErr(
				'The certificate must be PEM (-----BEGIN CERTIFICATE-----), leaf first, then the intermediates',
				'cert_pem'
			);
		}
		if (!body.key_pem.includes('PRIVATE KEY-----')) {
			return fieldErr(
				'The private key must be PEM (-----BEGIN PRIVATE KEY----- or an RSA/EC key)',
				'key_pem'
			);
		}
		if (body.key_pem.includes('WRONG')) {
			return fieldErr(
				`The private key doesn't belong to the certificate for ${domain}. Upload the key that was generated with this certificate's request.`,
				'key_pem'
			);
		}
		const now = new Date().toISOString();
		const row = {
			id: `cert-${Date.now()}`,
			domain,
			cert_type: 'custom',
			status: 'active',
			issued_at: now,
			expires_at: new Date(Date.now() + 365 * 86_400_000).toISOString(),
			last_error: null,
			last_attempt_at: now,
			next_retry_at: null,
			failure_count: 0,
			issuer: 'Example Corp Issuing CA',
			issuer_kind: 'uploaded' as const,
			last_renewed_at: now,
			challenge: null,
			serves: domain.startsWith('*.')
				? { kind: 'wildcard' as const, purpose: 'custom' as const, hosts: 0 }
				: { kind: 'unused' as const }
		};
		const i = certificates.findIndex((c) => c.domain === domain);
		if (i === -1) certificates.push(row);
		else certificates[i] = { ...row, serves: certificates[i].serves };
		return ok(certificates.find((c) => c.domain === domain) ?? null);
	}),

	http.delete('/api/v1/certs/custom/:domain', async ({ params }) => {
		await delay(LATENCY);
		const i = certificates.findIndex((c) => c.domain === params.domain && c.cert_type === 'custom');
		if (i === -1) return err(`No uploaded certificate for ${params.domain}`, 404);
		certificates.splice(i, 1);
		return ok({ deleted: params.domain });
	}),

	http.get('/api/v1/certs', async () => {
		await delay(LATENCY);
		return ok(certificates);
	}),

	http.post('/api/v1/certs/:domain/retry', async ({ params }) => {
		await delay(LATENCY);
		const cert = certificates.find((c) => c.domain === params.domain);
		if (!cert) return err(`No certificate for ${params.domain}`, 404);
		cert.next_retry_at = null;
		cert.last_attempt_at = new Date().toISOString();
		return ok({ message: 'Certificate retry queued', domain: cert.domain });
	}),

	http.put('/api/v1/apps/:id/health', async ({ params, request }) => {
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

	http.put('/api/v1/apps/:id/drain-timeout', async ({ params, request }) => {
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

	http.patch('/api/v1/apps/:id/resources', async ({ params, request }) => {
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

	http.post('/api/v1/apps/:id/volumes', async ({ params, request }) => {
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

	http.delete('/api/v1/apps/:id/volumes', async ({ params, request }) => {
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

	http.get('/api/v1/apps/:id/deployments/stats', async ({ params }) => {
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

	http.get('/api/v1/apps/:id/deployments', async ({ params }) => {
		await delay(LATENCY);
		return ok(deployments[params.id as string] ?? []);
	}),

	http.get(
		'/api/v1/apps/:id/deployments/:deploymentId/generated-dockerfile',
		async ({ params }) => {
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
		}
	),

	http.post('/api/v1/apps/:id/rollback', async ({ params, request }) => {
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
				}
			},
			{ status: 202 }
		);
	}),

	http.get('/api/v1/apps/:id/deployments/:deploymentId', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const d = (deployments[app.id] ?? []).find((x) => x.id === params.deploymentId);
		if (!d) return err(`deployment '${params.deploymentId}'`, 404);
		return ok(deploymentDetail(app, d));
	}),

	http.post('/api/v1/compose/validate', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { compose_content: string };
		const errors = mockComposeProblems(body.compose_content);
		return ok({
			valid: errors.length === 0,
			errors,
			services: mockComposeSpecs(body.compose_content)
		});
	}),

	// ── Compose services ──────────────────────────────────────────────────────

	http.get('/api/v1/apps/:id/services', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		if (app.source_type !== 'compose')
			return err(`Invalid input: '${app.name}' is not a Docker Compose app`, 400);
		return ok(composeServices[app.id] ?? []);
	}),

	http.put('/api/v1/apps/:id/services/:service', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const list = composeServices[app.id] ?? [];
		const svc = list.find((s) => s.name === params.service);
		if (!svc) return err(`Not found: service '${params.service}' in app '${app.name}'`, 404);
		const body = (await request.json()) as { published_port: number | null; domains: string[] };
		if (body.domains.length && body.published_port == null) {
			return err(
				`Invalid input: Publish a container port of service '${svc.name}' before routing domains to it`
			);
		}
		for (const d of body.domains) {
			const owner =
				apps.find((a) => a.domain === d && a.id !== app.id) ??
				apps.find(
					(a) => a.id !== app.id && (composeServices[a.id] ?? []).some((s) => s.domains.includes(d))
				);
			if (owner) return err(`Invalid input: ${d} is already used by the app '${owner.name}'`);
		}
		const portChanged = svc.published_port !== body.published_port;
		const updated: ComposeService = {
			...svc,
			published_port: body.published_port,
			host_port: body.published_port == null ? null : svc.host_port,
			redeploy_required: body.published_port != null && (portChanged || svc.redeploy_required),
			domains: [...new Set(body.domains)]
		};
		composeServices[app.id] = list.map((s) => (s.name === svc.name ? updated : s));
		return ok({
			service: updated,
			port_changed: portChanged,
			redeploy_required: updated.redeploy_required,
			warnings: []
		});
	}),

	// ── Apps deployed from a template ──
	http.get('/api/v1/apps/:id/template', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		if (!app.template_id)
			return err(`Invalid input: '${app.name}' wasn't deployed from a template`);
		return ok(templateStatus(app));
	}),

	http.post('/api/v1/apps/:id/template/domain-env', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app?.template_id) return err('App not found', 404);
		const body = (await request.json()) as { redeploy?: boolean };
		const updated = templateStatus(app).domain_env;
		if (templateDomainEnv[app.id] && app.domain) templateDomainEnv[app.id].host = app.domain;
		const d =
			body.redeploy && updated.length
				? startMockDeploy(app, { trigger_source: 'manual', trigger_detail: MOCK_USER.email })
				: null;
		return ok({ updated, deployment_id: d?.id ?? null });
	}),

	http.post('/api/v1/apps/:id/template/upgrade', async ({ params }) => {
		await delay(LATENCY * 4); // the snapshot
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx < 0 || !apps[idx].template_id) return err('App not found', 404);
		const status = templateStatus(apps[idx]);
		if (!status.update_available || !status.latest_version) {
			return err(`Conflict: ${apps[idx].name} is already on the catalog's version`, 409);
		}
		const from = apps[idx].template_version ?? null;
		const template = templates.find((t) => t.id === apps[idx].template_id)!;
		apps[idx] = {
			...apps[idx],
			template_version: status.latest_version,
			compose_content: template.compose
		};
		const d = startMockDeploy(apps[idx], {
			trigger_source: 'manual',
			trigger_detail: MOCK_USER.email
		});
		return HttpResponse.json(
			okBody({
				from_version: from,
				to_version: status.latest_version,
				snapshot_id: `snap_${Date.now()}`,
				deployment_id: d.id,
				added_env: [],
				credentials: [],
				upgrade_notes: status.upgrade_notes
			}),
			{ status: 202 }
		);
	}),

	http.delete('/api/v1/apps/:id/services/:service', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const list = composeServices[app.id] ?? [];
		const svc = list.find((s) => s.name === params.service);
		if (!svc) return err(`Not found: service '${params.service}' in app '${app.name}'`, 404);
		if (!svc.parked) {
			return err(
				`Invalid input: Service '${svc.name}' is in the compose file: remove it from the file and deploy instead`
			);
		}
		composeServices[app.id] = list.filter((s) => s.name !== svc.name);
		return ok(composeServices[app.id]);
	}),

	http.post('/api/v1/apps/:id/services/:service/:action', async ({ params }) => {
		await delay(LATENCY * 2);
		const app = apps.find((a) => a.id === params.id);
		if (!app) return err('App not found', 404);
		const list = composeServices[app.id] ?? [];
		const svc = list.find((s) => s.name === params.service);
		if (!svc) return err(`Not found: service '${params.service}'`, 404);
		const action = params.action as string;
		const status = action === 'stop' ? 'exited' : 'running';
		const updated: ComposeService = {
			...svc,
			status,
			exit_code: action === 'stop' ? 143 : null,
			health: svc.has_healthcheck && action !== 'stop' ? 'healthy' : null
		};
		composeServices[app.id] = list.map((s) => (s.name === svc.name ? updated : s));
		const past = action === 'restart' ? 'restarted' : action === 'stop' ? 'stopped' : 'started';
		return ok({ message: `Service ${svc.name} ${past}`, service: updated });
	}),

	http.get('/api/v1/apps/:id/services/:service/logs', async ({ params }) => {
		await delay(LATENCY);
		const now = Date.now();
		const lines = Array.from({ length: 40 }, (_, i) => {
			const ts = new Date(now - (40 - i) * 15_000).toISOString();
			const text =
				params.service === 'cache'
					? i % 10 === 0
						? '1:M * Background saving started by pid 42'
						: '1:M * DB saved on disk'
					: params.service === 'db'
						? 'LOG:  checkpoint complete: wrote 12 buffers (0.1%)'
						: `GET /products?page=${i % 5} 200 ${10 + (i % 7)}ms`;
			return `${ts} ${text}`;
		});
		return ok({ service: params.service, lines });
	}),

	http.put('/api/v1/apps/:id/compose', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as {
			compose_content?: string;
			source_url?: string;
			branch?: string;
			compose_path?: string;
		};
		if (body.source_url) {
			apps[idx] = {
				...apps[idx],
				compose_content: null,
				source_url: body.source_url,
				branch: body.branch ?? 'main',
				compose_path: body.compose_path ?? 'docker-compose.yml'
			};
		} else {
			const problems = mockComposeProblems(body.compose_content ?? '');
			if (problems.length) return err(`Compose file rejected: ${problems[0]}`, 400);
			const specs = mockComposeSpecs(body.compose_content ?? '');
			const old = composeServices[apps[idx].id] ?? [];
			composeServices[apps[idx].id] = specs.map((spec) => {
				const prev = old.find((s) => s.name === spec.name);
				return prev
					? { ...prev, ...spec }
					: {
							...spec,
							published_port: null,
							host_port: null,
							redeploy_required: false,
							domains: [],
							status: 'pending',
							health: null,
							exit_code: null,
							container_id: null,
							running_image: null,
							image_digest: null,
							cpu: null,
							mem: null
						};
			});
			apps[idx] = {
				...apps[idx],
				compose_content: body.compose_content ?? '',
				source_url: null,
				compose_path: null
			};
		}
		return ok({
			message: 'Compose file saved. Deploy to apply it.',
			services: composeServices[apps[idx].id] ?? []
		});
	}),

	// ── Git Integration ───────────────────────────────────────────────────────

	http.get('/api/v1/apps/:id/deploy-key', async ({ params }) => {
		await delay(LATENCY);
		const key = MOCK_DEPLOY_KEYS[params.id as string];
		if (!key) return err('No deploy key found', 404);
		return ok(key);
	}),

	http.post('/api/v1/apps/:id/deploy-key', async ({ params }) => {
		await delay(LATENCY);
		const newKey = {
			app_id: params.id as string,
			public_key: `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA${Date.now()} tond@deploy`
		};
		MOCK_DEPLOY_KEYS[params.id as string] = newKey;
		return ok(newKey);
	}),

	http.get('/api/v1/apps/:id/webhook', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		return ok(webhookConfig(app));
	}),

	http.put('/api/v1/apps/:id/webhook-secret', async ({ params, request }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const body = (await request.json()) as { secret?: unknown };
		const secret = typeof body.secret === 'string' ? body.secret.trim() : '';
		if (secret.length < 16) {
			return err(
				'webhook secret must be at least 16 characters (rotate it to have one generated)',
				400
			);
		}
		webhookSecrets[app.id] = secret;
		return ok(webhookConfig(app));
	}),

	http.get('/api/v1/apps/:id/webhook-secret', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		webhookConfig(app);
		return ok({ secret: webhookSecrets[app.id] });
	}),

	http.post('/api/v1/apps/:id/webhook-secret/rotate', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		webhookSecrets[app.id] = randomHex(32);
		return ok({ secret: webhookSecrets[app.id] });
	}),

	http.put('/api/v1/apps/:id/auto-deploy', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { auto_deploy: boolean; watch_branch: string | null };
		apps[idx] = { ...apps[idx], ...body, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	http.put('/api/v1/apps/:id/source', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { source_type: string; source_url: string | null };
		apps[idx] = { ...apps[idx], ...body, updated_at: new Date().toISOString() };
		return ok(apps[idx]);
	}),

	// ── Registry Credentials (Phase 4.3) ──────────────────────────────────────

	http.get('/api/v1/registry-credentials', async () => {
		await delay(LATENCY);
		return ok(registryCredentials);
	}),

	http.post('/api/v1/registry-credentials', async ({ request }) => {
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

	http.delete('/api/v1/registry-credentials/:id', async ({ params }) => {
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

	http.put('/api/v1/apps/:id/registry-credential', async ({ params, request }) => {
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

	http.get('/api/v1/apps/:id/previews', async ({ params }) => {
		await delay(LATENCY);
		return ok(previews[params.id as string] ?? []);
	}),

	http.post('/api/v1/apps/:id/preview', async ({ params, request }) => {
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
			return err('pr_url must be an https:// URL', 400);
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

	http.delete('/api/v1/apps/:id/previews/:previewId', async ({ params }) => {
		await delay(LATENCY);
		const appId = params.id as string;
		previews = {
			...previews,
			[appId]: (previews[appId] ?? []).filter((p) => p.id !== params.previewId)
		};
		return ok(null);
	}),

	http.put('/api/v1/apps/:id/preview-config', async ({ params, request }) => {
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

	http.get('/api/v1/projects/:id/previews', async ({ params }) => {
		await delay(LATENCY);
		const projectId = params.id as string;
		return ok(projectPreviews[projectId] ?? []);
	}),

	http.post('/api/v1/projects/:id/previews', async ({ params, request }) => {
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

	http.delete('/api/v1/projects/:id/previews/:previewId', async ({ params }) => {
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
	http.get('/api/v1/apps/:id/metrics', async ({ request, params }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '1h';
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		const measured = app?.status === 'running' || app?.status === 'unhealthy';
		return ok(makeMockMetrics(range, !measured));
	}),

	http.get('/api/v1/databases/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h'));
	}),

	http.get('/api/v1/cron/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h', true));
	}),

	http.get('/api/v1/previews/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockMetrics(new URL(request.url).searchParams.get('range') ?? '1h'));
	}),

	// ── Request metrics (tond-proxy traffic) ──────────────────────────────────

	// Apps that are not running (and workers without a domain) have no traffic.
	http.get('/api/v1/apps/:id/requests', async ({ request, params }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '24h';
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		const served = (app.status === 'running' || app.status === 'unhealthy') && !!app.domain;
		return ok(makeMockRequestMetrics(range, { empty: !served }));
	}),

	http.get('/api/v1/previews/:id/requests', async ({ request }) => {
		await delay(LATENCY);
		const range = new URL(request.url).searchParams.get('range') ?? '24h';
		return ok(makeMockRequestMetrics(range, { scale: 0.3, withDeploys: false }));
	}),

	http.get('/api/v1/system/requests', async ({ request }) => {
		await delay(LATENCY);
		return ok(makeMockTrafficOverview(new URL(request.url).searchParams.get('range') ?? '24h'));
	}),

	// ── Access logs ───────────────────────────────────────────────────────────

	http.get('/api/v1/apps/:id/access-log', async ({ request, params }) => {
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

	http.put('/api/v1/apps/:id/access-log', async ({ request, params }) => {
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

	http.delete('/api/v1/apps/:id/access-log', async ({ params }) => {
		await delay(LATENCY);
		const id = params.id as string;
		if (accessLogs[id]) accessLogs = { ...accessLogs, [id]: { ...accessLogs[id], lines: [] } };
		return ok(null);
	}),

	http.get('/api/v1/apps/:id/access-log/download', async ({ params }) => {
		const lines = accessLogs[params.id as string]?.lines ?? [];
		return new HttpResponse(lines.map((l) => `${l}\n`).join(''), {
			headers: {
				'Content-Type': 'text/plain; charset=utf-8',
				'Content-Disposition': 'attachment; filename="access.log"'
			}
		});
	}),

	http.get('/api/v1/system/self', async () => {
		await delay(LATENCY);
		return ok(makeMockSystemSelf());
	}),

	http.get('/api/v1/system/metrics', async () => {
		await delay(LATENCY);
		return ok(makeMockSystemMetrics());
	}),

	// ── Settings ──────────────────────────────────────────────────────────────

	http.get('/api/v1/settings', async () => {
		await delay(LATENCY);
		return ok(MOCK_SETTINGS);
	}),

	http.get('/api/v1/onboarding/status', async () => {
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

	http.get('/api/v1/apps/:id/events', async ({ params, request }) => {
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

	http.get('/api/v1/audit-log/shipping', async () => {
		await delay(LATENCY);
		return ok(MOCK_AUDIT_SHIPPING);
	}),

	http.put('/api/v1/audit-log/shipping', async ({ request }) => {
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

	http.post('/api/v1/audit-log/shipping/test', async ({ request }) => {
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

	http.get('/api/v1/settings/retention', async () => {
		await delay(LATENCY);
		return ok(MOCK_RETENTION);
	}),

	http.put('/api/v1/settings/retention', async ({ request }) => {
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

	http.put('/api/v1/settings', async ({ request }) => {
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

	http.get('/api/v1/alerts/kinds', async () => {
		await delay(LATENCY);
		return ok(MOCK_ALERT_KINDS);
	}),

	http.get('/api/v1/alerts/active', async () => {
		await delay(LATENCY);
		return ok(activeAlerts.filter((a) => alertRules.find((r) => r.id === a.rule_id)?.enabled));
	}),

	http.get('/api/v1/alerts/history', async ({ request }) => {
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

	http.get('/api/v1/alerts/history/:id', async ({ params }) => {
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

	http.post('/api/v1/alerts/history/:id/acknowledge', async ({ params }) => {
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

	http.get('/api/v1/alerts/settings', async () => {
		await delay(LATENCY);
		return ok(alertSettings);
	}),

	http.put('/api/v1/alerts/settings', async ({ request }) => {
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

	http.get('/api/v1/alerts', async () => {
		await delay(LATENCY);
		return ok(alertRules);
	}),

	http.get('/api/v1/alerts/:id', async ({ params }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		return ok(rule);
	}),

	http.post('/api/v1/alerts', async ({ request }) => {
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
		return HttpResponse.json({ success: true, data: newRule }, { status: 201 });
	}),

	http.patch('/api/v1/alerts/:id', async ({ params, request }) => {
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

	http.post('/api/v1/alerts/:id/test', async ({ params }) => {
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

	http.post('/api/v1/alerts/:id/snooze', async ({ params, request }) => {
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

	http.post('/api/v1/alerts/:id/unsnooze', async ({ params, request }) => {
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

	http.delete('/api/v1/alerts/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = alertRules.some((r) => r.id === params.id);
		if (!exists) return err('Alert rule not found', 404);
		alertRules = alertRules.filter((r) => r.id !== params.id);
		activeAlerts = activeAlerts.filter((a) => a.rule_id !== params.id);
		return ok({ deleted: params.id });
	}),

	// ── Audit Log ─────────────────────────────────────────────────────────────

	// Same filters as the server, applied before the limit.
	http.get('/api/v1/audit-log', async ({ request }) => {
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

	http.get('/api/v1/audit-log/verify', async () => {
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

	http.get('/api/v1/apps/:id/logs', async () => {
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
				{ ts: new Date().toISOString(), line: '[mock] GET /api/v1/users 200 OK' }
			]
		});
	}),

	// ── Managed Databases (Phase 6.3) ─────────────────────────────────────────
	// Passwords and connection-string overrides are write-only, like the
	// server: responses carry has_password flags and masked strings, and the
	// plaintext comes only from the (audited) credentials reveal.

	http.get('/api/v1/databases', async () => {
		await delay(LATENCY);
		return ok(databases);
	}),

	http.get('/api/v1/databases/catalog', async () => {
		await delay(LATENCY);
		return ok(MOCK_DB_CATALOG);
	}),

	http.get('/api/v1/databases/:id/backup-schedule', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err(`database '${params.id}'`, 404);
		return ok(mockBackupStatus(db));
	}),

	http.put('/api/v1/databases/:id/backup-schedule', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err(`database '${params.id}'`, 404);
		const body = (await request.json()) as { schedule: string | null; retention?: number };
		const schedule = body.schedule?.trim() || null;
		if (schedule && schedule.split(/\s+/).length !== 5 && !schedule.startsWith('@')) {
			return err(
				`Invalid backup schedule: a schedule needs 5 fields (minute hour day-of-month month day-of-week), got ${schedule.split(/\s+/).length}`
			);
		}
		const retention = body.retention ?? databases[idx].backup_retention;
		if (retention < 1 || retention > 365) {
			return err(
				'Keep between 1 and 365 backups. A schedule that keeps none would delete each backup right after taking it.'
			);
		}
		databases[idx] = { ...databases[idx], backup_schedule: schedule, backup_retention: retention };
		return ok(mockBackupStatus(databases[idx]));
	}),

	http.get('/api/v1/databases/:id/extensions', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		if (db.db_type !== 'postgres') return err('Extensions are a PostgreSQL feature.');
		if (db.status !== 'running') {
			return err(`Start '${db.name}' first: extensions are read from the running database.`);
		}
		const entry = mockCatalogEntry(db.db_type, db.variant);
		const installed = dbExtensions[db.id] ?? {};
		const comments: Record<string, string> = {
			vector: 'vector data type and ivfflat and hnsw access methods',
			postgis: 'PostGIS geometry and geography spatial types and functions',
			pg_stat_statements: 'track planning and execution statistics of all SQL statements executed',
			pg_trgm: 'text similarity measurement and index searching based on trigrams',
			pgcrypto: 'cryptographic functions',
			'uuid-ossp': 'generate universally unique identifiers (UUIDs)',
			citext: 'data type for case-insensitive character strings'
		};
		const list: DatabaseExtension[] = (entry?.extensions ?? []).map((name) => ({
			name,
			available: true,
			installed_version: installed[name] ?? null,
			default_version: name === 'vector' ? '0.8.1' : '1.6',
			comment: comments[name] ?? null
		}));
		return ok(list);
	}),

	http.post('/api/v1/databases/:id/extensions', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		const { name } = (await request.json()) as { name: string };
		const offered = mockCatalogEntry(db.db_type, db.variant)?.extensions ?? [];
		if (!offered.includes(name)) {
			return err(
				`'${name}' isn't one of the extensions offered for this database: ${offered.join(', ')}`
			);
		}
		(dbExtensions[db.id] ??= {})[name] = name === 'vector' ? '0.8.1' : '1.6';
		return ok({ message: `${name} is enabled in ${db.name}.` });
	}),

	http.post('/api/v1/databases/:id/upgrade', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		if (currentMockUser().role !== 'admin') {
			return err('Forbidden: only admins can upgrade a database to a new version', 403);
		}
		const db = databases[idx];
		const { version } = (await request.json()) as { version: string };
		if (db.upgrade?.status === 'running') return err('An upgrade is already running.');
		if (db.status !== 'running') {
			return err('Start the database first: the upgrade copies its data while it runs.');
		}
		if (!mockNewer(version, db.version)) {
			return err(
				`Downgrades aren't possible: version ${version} can't read data written by ${db.version}. Restore a backup taken before the upgrade into a new database instead.`
			);
		}
		const upgrade: DbUpgradeState = {
			status: 'running',
			from_version: db.version,
			to_version: version,
			from_image: db.image_used ?? '',
			to_image: mockImage(db.db_type, db.variant, version),
			step: 'Starting',
			backup_id: null,
			started_at: new Date().toISOString(),
			finished_at: null,
			error: null
		};
		databases[idx] = { ...db, upgrade };
		runMockUpgrade(db.id);
		return HttpResponse.json(okBody(upgrade), { status: 202 });
	}),

	http.post('/api/v1/databases/:id/upgrade/confirm', async ({ params }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const db = databases[idx];
		if (db.upgrade?.status !== 'awaiting_confirmation') {
			return err('No upgrade is waiting for confirmation.');
		}
		databases[idx] = { ...db, upgrade: null };
		return ok({ message: `${db.name} stays on version ${db.version}. The old data was deleted.` });
	}),

	http.post('/api/v1/databases/:id/upgrade/rollback', async ({ params }) => {
		await delay(LATENCY * 3);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const db = databases[idx];
		const u = db.upgrade;
		if (u?.status !== 'awaiting_confirmation') {
			return err('No upgrade is waiting for confirmation.');
		}
		databases[idx] = { ...db, version: u.from_version, image_used: u.from_image, upgrade: null };
		return ok({
			message: `${db.name} runs version ${u.from_version} again, with its data from before the upgrade.`
		});
	}),

	http.post('/api/v1/databases/:id/upgrade/dismiss', async ({ params }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		if (databases[idx].upgrade?.status === 'failed') {
			databases[idx] = { ...databases[idx], upgrade: null };
		}
		return ok(null);
	}),

	http.post('/api/v1/snapshots/:id/restore-new-database', async ({ params, request }) => {
		await delay(LATENCY * 5);
		const snap = snapshots.find((s) => s.id === params.id);
		if (!snap) return err('Snapshot not found', 404);
		const body = (await request.json()) as { source: string; name: string };
		const source = databases.find((d) => d.name === body.source);
		if (!source) {
			return err(
				`This snapshot predates restoring into a new database and '${body.source}' no longer exists`
			);
		}
		const name = body.name.trim();
		if (!/^[A-Za-z0-9][A-Za-z0-9-]*$/.test(name)) {
			return err('Database name must be alphanumeric (hyphens allowed)');
		}
		if (databases.some((d) => d.name === name)) {
			return err(`A database named '${name}' already exists. Choose another name.`);
		}
		const isRedisLike = source.db_type === 'redis' || source.db_type === 'keydb';
		const copy: ManagedDatabase = {
			...source,
			id: `db_${Date.now()}`,
			name,
			host_port: 5400 + databases.length + 1,
			status: 'running',
			db_name: isRedisLike ? null : `tond_${name.replace(/-/g, '_')}`,
			db_user: isRedisLike ? 'default' : `tond_${name.replace(/-/g, '_')}`,
			public_access: false,
			allowlist: [],
			tls: false,
			backup_schedule: null,
			upgrade: null,
			custom_external_url: null,
			custom_internal_url: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		dbSecrets[copy.id] = { db_password: randomHex(12), root_password: null };
		databases = [...databases, copy];
		return HttpResponse.json(
			okBody({
				message: `Restored ${source.name} into the new database ${name}. ${source.name} was not changed.`,
				database: copy
			}),
			{ status: 201 }
		);
	}),

	// ── Git tokens (GitLab, Gitea/Forgejo) ──
	http.get('/api/v1/gitlab-tokens', async () => {
		await delay(LATENCY);
		return ok(gitlabTokens);
	}),

	http.post('/api/v1/gitlab-tokens', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as { name: string; token: string; gitlab_url?: string };
		if (!body.name?.trim() || !body.token?.trim()) return err('name is required');
		if (body.token.trim() === 'bad') {
			return err('GitLab token validation failed (HTTP 401 Unauthorized)');
		}
		const token: GitLabToken = {
			id: `gl_${Date.now()}`,
			name: body.name.trim(),
			gitlab_url: (body.gitlab_url || 'https://gitlab.com').replace(/\/+$/, ''),
			username: 'dev',
			created_at: new Date().toISOString()
		};
		gitlabTokens = [...gitlabTokens, token];
		return HttpResponse.json(okBody(token), { status: 201 });
	}),

	http.delete('/api/v1/gitlab-tokens/:id', async ({ params }) => {
		await delay(LATENCY);
		gitlabTokens = gitlabTokens.filter((t) => t.id !== params.id);
		apps = apps.map((a) => (a.gitlab_token_id === params.id ? { ...a, gitlab_token_id: null } : a));
		return ok(null);
	}),

	http.get('/api/v1/gitea-tokens', async () => {
		await delay(LATENCY);
		return ok(giteaTokens);
	}),

	http.post('/api/v1/gitea-tokens', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as { name: string; token: string; base_url: string };
		if (!body.name?.trim() || !body.token?.trim()) {
			return err('A name and a token are required');
		}
		if (!/^https?:\/\/[^/@\s]+/.test(body.base_url?.trim() ?? '')) {
			return err(`The instance URL must start with https:// (or http://)`);
		}
		if (body.token.trim() === 'bad') {
			return err(
				`${body.base_url} refused the token (HTTP 401). Create one under Settings → Applications with repository and issue write access.`
			);
		}
		const token: GiteaToken = {
			id: `gt_${Date.now()}`,
			name: body.name.trim(),
			base_url: body.base_url.trim().replace(/\/+$/, ''),
			username: 'dev',
			created_at: new Date().toISOString()
		};
		giteaTokens = [...giteaTokens, token];
		return HttpResponse.json(okBody(token), { status: 201 });
	}),

	http.delete('/api/v1/gitea-tokens/:id', async ({ params }) => {
		await delay(LATENCY);
		giteaTokens = giteaTokens.filter((t) => t.id !== params.id);
		apps = apps.map((a) => (a.gitea_token_id === params.id ? { ...a, gitea_token_id: null } : a));
		return ok(null);
	}),

	http.put('/api/v1/apps/:id/gitlab-token', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id || a.name === params.id);
		if (idx === -1) return err('App not found', 404);
		const { gitlab_token_id } = (await request.json()) as { gitlab_token_id: string | null };
		apps[idx] = { ...apps[idx], gitlab_token_id };
		return ok(apps[idx]);
	}),

	http.put('/api/v1/apps/:id/gitea-token', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id || a.name === params.id);
		if (idx === -1) return err('App not found', 404);
		const { gitea_token_id } = (await request.json()) as { gitea_token_id: string | null };
		apps[idx] = { ...apps[idx], gitea_token_id };
		return ok(apps[idx]);
	}),

	http.post('/api/v1/databases', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			db_type: string;
			project_id?: string;
			memory_limit?: number;
			public_access?: boolean;
			version?: string;
			variant?: string;
			backup_schedule?: string;
		};
		const variant = body.variant ?? '';
		const entry = mockCatalogEntry(body.db_type, variant);
		if (!entry) return err(`Unknown variant '${variant}' for ${body.db_type}`);
		const version = body.version || entry.default_version;
		if (!entry.versions.includes(version)) {
			return err(
				`Version ${version} isn't available. Choose one of: ${entry.versions.join(', ')}.`
			);
		}
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
			image_used: mockImage(body.db_type, variant, version),
			custom_image: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString(),
			public_access: body.public_access ?? false,
			version,
			variant,
			backup_schedule: body.backup_schedule?.trim() || null,
			backup_retention: 7,
			allowlist: [],
			tls: false,
			upgrade: null
		};
		dbSecrets[newDb.id] = {
			db_password: randomHex(12),
			root_password: isMysqlLike ? randomHex(12) : null
		};
		databases = [...databases, newDb];
		// The creator gets the generated credentials once.
		return ok({ ...dbDetail(newDb), credentials: dbCredentials(newDb) });
	}),

	http.get('/api/v1/databases/:id/access', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		return ok(mockDatabaseAccess(db));
	}),

	http.put('/api/v1/databases/:id/links/:app', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('database', 404);
		const app = apps.find((a) => a.id === params.app || a.name === params.app);
		if (!app) return err('App not found', 404);
		if (app.source_type === 'compose') {
			return err(
				`'${app.name}' is a compose app: compose apps run on their own compose network and can't be linked.`
			);
		}
		if (db.project_id && db.project_id === app.project_id) {
			return err(`'${app.name}' is in the same project as '${db.name}' and can already reach it.`);
		}
		if (!databaseLinks.some((l) => l.database_id === db.id && l.app_id === app.id)) {
			databaseLinks = [...databaseLinks, { database_id: db.id, app_id: app.id }];
		}
		return ok(mockDatabaseAccess(db));
	}),

	http.delete('/api/v1/databases/:id/links/:app', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('database', 404);
		const app = apps.find((a) => a.id === params.app || a.name === params.app);
		if (!app) return err('App not found', 404);
		databaseLinks = databaseLinks.filter((l) => !(l.database_id === db.id && l.app_id === app.id));
		return ok(mockDatabaseAccess(db));
	}),

	http.get('/api/v1/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		return ok(dbDetail(db));
	}),

	http.get('/api/v1/databases/:id/credentials', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		if (currentMockUser().role !== 'admin') {
			return err('Forbidden: only admins can reveal database credentials', 403);
		}
		return ok(dbCredentials(db));
	}),

	http.get('/api/v1/databases/:id/backups', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err(`database '${params.id}' not found`, 404);
		return ok(snapshots.filter((s) => s.scope === 'full' || s.scope === `database:${db.name}`));
	}),

	http.post('/api/v1/databases/:id/backups', async ({ params }) => {
		await delay(LATENCY * 4);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err(`database '${params.id}' not found`, 404);
		const snap = mockDatabaseSnapshot(db.name, 'manual');
		snapshots = [snap, ...snapshots];
		return HttpResponse.json({ success: true, data: snap }, { status: 201 });
	}),

	http.delete('/api/v1/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		const gone = databases.find((d) => d.id === params.id || d.name === params.id);
		databases = databases.filter((d) => d.id !== params.id && d.name !== params.id);
		if (gone) databaseLinks = databaseLinks.filter((l) => l.database_id !== gone.id);
		return ok(null);
	}),

	http.patch('/api/v1/databases/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = databases.findIndex((d) => d.id === params.id || d.name === params.id);
		if (idx === -1) return err('Database not found', 404);
		const body = (await request.json()) as {
			custom_external_url?: string | null;
			custom_internal_url?: string | null;
			custom_image?: string | null;
			memory_limit?: number | null;
			public_access?: boolean;
			allowlist?: string[];
			tls?: boolean;
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
		if (body.allowlist !== undefined) {
			const cleaned: string[] = [];
			for (const raw of body.allowlist) {
				const entry = raw.trim();
				if (!entry) continue;
				const [addr, prefix] = entry.split('/');
				const octets = addr.split('.');
				const validIp =
					octets.length === 4 && octets.every((o) => /^\d+$/.test(o) && Number(o) <= 255);
				const validPrefix = prefix === undefined || (/^\d+$/.test(prefix) && Number(prefix) <= 32);
				if (addr.includes(':')) {
					return err(
						`'${entry}' is IPv6. Public databases are published on IPv4 only; allow IPv4 addresses.`
					);
				}
				if (!validIp || !validPrefix) {
					return err(`'${entry}' isn't an IP address or a CIDR range like 203.0.113.0/24`);
				}
				const canonical = prefix === '32' ? addr : entry;
				if (!cleaned.includes(canonical)) cleaned.push(canonical);
			}
			updated.allowlist = cleaned;
		}
		if (body.tls !== undefined) {
			if (body.tls && !mockCatalogEntry(updated.db_type, updated.variant)?.supports_tls) {
				return err(
					`${updated.db_type} doesn't support TLS in Tond; use an SSH tunnel to encrypt connections.`
				);
			}
			updated.tls = body.tls;
		}
		updated.updated_at = new Date().toISOString();
		databases[idx] = updated;
		return ok(dbDetail(updated));
	}),

	http.patch('/api/v1/databases/:id/credentials', async ({ params, request }) => {
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
		if (changed.length === 0) return err('Nothing to change: enter a new password.');
		if (
			(current.db_type === 'redis' || current.db_type === 'keydb') &&
			nextPw &&
			/[\s"'\\#]/.test(nextPw)
		) {
			return err(
				"Invalid input: Redis password may not contain spaces, quotes, backslashes or '#'"
			);
		}
		if (current.status !== 'running') {
			return err(
				`Start '${current.name}' first: the password is changed inside the running database. Nothing was changed.`
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
		const appsToUpdate = apps
			.filter((a) => a.project_id && a.project_id === current.project_id)
			.slice(0, 2)
			.map((a) => ({ id: a.id, name: a.name, keys: ['DATABASE_URL'] }));
		return ok({ database: dbDetail(updated), changed, apps_to_update: appsToUpdate });
	}),

	http.post('/api/v1/databases/:id/stop', async ({ params }) => {
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

	http.post('/api/v1/databases/:id/start', async ({ params }) => {
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

	http.post('/api/v1/databases/:id/redeploy', async ({ params }) => {
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

	http.get('/api/v1/cron', async () => {
		await delay(LATENCY);
		return ok(cronJobs);
	}),

	http.get('/api/v1/cron/schedule-preview', async ({ request }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const schedule = url.searchParams.get('schedule') ?? '';
		const timezone = url.searchParams.get('timezone') || 'UTC';
		const count = Number(url.searchParams.get('count') ?? 5);
		const runs = mockNextRuns(schedule, count);
		if (!runs) return err(`Invalid cron expression '${schedule}'`, 400);
		return ok({ next_runs: runs, timezone });
	}),

	http.post('/api/v1/cron', async ({ request }) => {
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

	http.get('/api/v1/cron/:id', async ({ params }) => {
		await delay(LATENCY);
		const job = cronJobs.find((j) => j.id === params.id || j.name === params.id);
		if (!job) return err('Cron job not found', 404);
		return ok(job);
	}),

	http.patch('/api/v1/cron/:id', async ({ params, request }) => {
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

	http.delete('/api/v1/cron/:id', async ({ params }) => {
		await delay(LATENCY);
		cronJobs = cronJobs.filter((j) => j.id !== params.id && j.name !== params.id);
		return ok(null);
	}),

	http.post('/api/v1/cron/:id/trigger', async ({ params }) => {
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

	http.get('/api/v1/cron/:id/runs', async ({ params, request }) => {
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
	// Simulates /api/v1/cron/{id}/runs/stream.
	// On connect: sends a snapshot of the most recent run for the job (or null).
	// Every 8 s: emits a new simulated completed run frame.

	cronRunStream.addEventListener('connection', ({ client }) => {
		// Extract job id from the URL path: /api/v1/cron/<id>/runs/stream
		const match = client.url.pathname.match(/\/api\/v1\/cron\/([^/]+)\/runs\/stream/);
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
	// Simulates /api/v1/apps/{id}/logs/stream.
	// On connect: sends a snapshot of 5 seed lines.
	// Every 2 s: sends one incremental container_log line.

	appLogStream.addEventListener('connection', ({ client, params }) => {
		// Compose apps: every service's output, tagged with the service.
		const services = composeServices[params.id as string];
		if (services) {
			const names = services.filter((s) => s.status === 'running').map((s) => s.name);
			const lineFor = (service: string, n: number) =>
				service === 'cache'
					? `1:M * ${n} changes in 60 seconds. Saving...`
					: service === 'db'
						? `LOG:  checkpoint starting: time (${n})`
						: `GET /products?page=${n % 5} 200 ${8 + (n % 9)}ms`;
			client.send(
				JSON.stringify({
					type: 'snapshot',
					deploy_logs: [
						{
							level: 'info',
							message: 'Deployed successfully! 5 service(s) running',
							ts: new Date().toISOString()
						}
					],
					container_logs: names.flatMap((service, i) => [
						{
							ts: new Date(Date.now() - (10 - i) * 1000).toISOString(),
							service,
							line: lineFor(service, i)
						}
					])
				})
			);
			let counter = 0;
			const timer = setInterval(() => {
				counter += 1;
				const service = names[counter % names.length];
				client.send(
					JSON.stringify({
						type: 'container',
						ts: new Date().toISOString(),
						service,
						line: lineFor(service, counter)
					})
				);
			}, 1500);
			client.addEventListener('close', () => clearInterval(timer));
			return;
		}
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
	// Simulates /api/v1/system/logs/stream.
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

	http.get('/api/v1/github-apps', async () => {
		await delay(LATENCY);
		return ok(githubApps);
	}),

	http.post('/api/v1/github-apps/complete', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { state?: string | null };
		if (!body.state) return err('missing_state', 400);
		return ok({ completed: true });
	}),

	http.post('/api/v1/github-apps/manifest', async () => {
		await delay(LATENCY);
		// In mock mode: simulate the manifest flow — return a github_url that points
		// back to the integrations page (triggering the ?mock_github_connected=1 handler)
		// and a dummy manifest string (never actually POSTed to GitHub in dev).
		return ok({
			github_url: '/integrations/github?mock_github_connected=1',
			manifest: '{}'
		});
	}),

	http.post('/api/v1/github-apps/:id/install', async ({ params }) => {
		await delay(LATENCY);
		const app = githubApps.find((a) => a.id === params.id);
		if (!app) return err('GitHub App not found', 404);
		// The real server returns a single-use, admin-bound state on the URL.
		return ok({
			url: `https://github.com/apps/${encodeURIComponent(app.slug)}/installations/new?state=${randomHex(32)}`
		});
	}),

	http.patch('/api/v1/github-apps/:id', async ({ params, request }) => {
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

	http.get('/api/v1/github-apps/:id/repos', async ({ params }) => {
		await delay(LATENCY);
		const repos = MOCK_GITHUB_REPOS[params.id as string] ?? [];
		return ok(repos);
	}),

	http.get('/api/v1/github-apps/:id/repos/:owner/:repo/branches', async ({ params }) => {
		await delay(LATENCY);
		const key = `${params.owner}/${params.repo}`;
		const branches = MOCK_GITHUB_BRANCHES[key] ?? [{ name: 'main' }];
		return ok(branches);
	}),

	http.put('/api/v1/apps/:id/github-app', async ({ params, request }) => {
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

	http.get('/api/v1/apps/:id/github-prs', async ({ params }) => {
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

	http.delete('/api/v1/github-apps/:id', async ({ params }) => {
		await delay(LATENCY);
		githubApps = githubApps.filter((a) => a.id !== params.id);
		return ok(null);
	}),

	// ── Notification Integrations ──────────────────────────────────────────────

	http.get('/api/v1/integrations', async () => {
		await delay(LATENCY);
		return ok(integrations);
	}),

	http.post('/api/v1/integrations', async ({ request }) => {
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
		return HttpResponse.json({ success: true, data: newIntegration }, { status: 201 });
	}),

	http.get('/api/v1/integrations/:id', async ({ params }) => {
		await delay(LATENCY);
		const integration = integrations.find((i) => i.id === params.id);
		if (!integration) return err('Integration not found', 404);
		return ok(integration);
	}),

	http.patch('/api/v1/integrations/:id', async ({ params, request }) => {
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

	http.delete('/api/v1/integrations/:id', async ({ params }) => {
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

	http.post('/api/v1/integrations/:id/test', async ({ params }) => {
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
	// Simulates /api/v1/databases/logs/stream.
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
	// Simulates /api/v1/databases/{id}/logs/stream.
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

	http.get('/api/v1/storage-providers', async () => {
		await delay(LATENCY);
		return ok(storageProviders);
	}),

	http.post('/api/v1/storage-providers', async ({ request }) => {
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

	http.patch('/api/v1/storage-providers/:id', async ({ params, request }) => {
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

	http.delete('/api/v1/storage-providers/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = storageProviders.some((p) => p.id === params.id);
		if (!exists) return err('Storage provider not found', 404);
		storageProviders = storageProviders.filter((p) => p.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/v1/storage-providers/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const p = storageProviders.find((sp) => sp.id === params.id);
		if (!p) return err('Storage provider not found', 404);
		return ok({ ok: true, message: `Connected to ${p.config.bucket} successfully.` });
	}),

	// ── Snapshots ────────────────────────────────────────────────────────────

	http.get('/api/v1/snapshots', async ({ request }) => {
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

	http.post('/api/v1/snapshots', async ({ request }) => {
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

	http.delete('/api/v1/snapshots/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = snapshots.some((s) => s.id === params.id);
		if (!exists) return err('Snapshot not found', 404);
		snapshots = snapshots.filter((s) => s.id !== params.id);
		return ok({ deleted: params.id });
	}),

	http.post('/api/v1/snapshots/:id/restore', async ({ params }) => {
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

	http.post('/api/v1/snapshots/:id/restore-app', async ({ params, request }) => {
		await delay(LATENCY * 2);
		const snap = snapshots.find((s) => s.id === params.id);
		if (!snap) return err('Snapshot not found', 404);
		const body = (await request.json()) as any;
		return ok({ message: `App '${body.app_name}' rolled back successfully.` });
	}),

	http.post('/api/v1/snapshots/:id/restore-database', async ({ params, request }) => {
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

	// ── Restore from storage ─────────────────────────────────────────────────

	http.get('/api/v1/snapshots/remote', async ({ request }) => {
		await delay(LATENCY * 2);
		const id =
			new URL(request.url).searchParams.get('provider_id') ?? backupConfig.storage_provider_id;
		if (!id) {
			return err(
				'No storage provider given and none is selected in Backup settings. Pass provider_id.',
				400
			);
		}
		if (!storageProviders.some((p) => p.id === id)) {
			return err('Storage provider not found', 404);
		}
		return ok(
			remoteSnapshots
				.filter((s) => s.provider_id === id)
				.map((s) => ({ ...s, imported: s.imported || snapshots.some((l) => l.id === s.id) }))
		);
	}),

	http.post('/api/v1/snapshots/remote/:id/import', async ({ params, request }) => {
		await delay(LATENCY * 3);
		const body = (await request.json().catch(() => ({}))) as { provider_id?: string };
		const providerId = body.provider_id ?? backupConfig.storage_provider_id;
		const remote = remoteSnapshots.find((s) => s.id === params.id && s.provider_id === providerId);
		if (!remote) return err('Snapshot not found in remote storage', 404);
		if (!remote.readable) {
			return HttpResponse.json(
				errorBody(
					"This snapshot was encrypted with a different TOND_SECRET_KEY, so this server can't read it.",
					409,
					'SNAPSHOT_KEY_MISMATCH',
					{
						hint: 'Set TOND_SECRET_KEY in /opt/tond/data/tond.env to the key of the server that made it, restart tond-server and try again.'
					}
				),
				{ status: 409 }
			);
		}
		const existing = snapshots.find((s) => s.id === remote.id);
		if (existing) return ok(existing);
		const snap: Snapshot = {
			id: remote.id,
			name: remote.name ?? remote.id,
			status: 'completed',
			trigger: remote.trigger ?? 'manual',
			scope: remote.scope ?? 'full',
			trigger_app: null,
			deployment_id: null,
			parent_snapshot_id: null,
			created_at: remote.created_at,
			completed_at: remote.created_at,
			delta_bytes: null,
			total_bytes: null,
			local_path: null,
			remote_key: `snapshots/${remote.id}.tondsnap`,
			storage_provider_id: remote.provider_id,
			checksum_sha256: 'f'.repeat(64),
			error_message: null,
			notes: null,
			remote_status: 'uploaded',
			remote_error: null,
			archive_bytes: remote.archive_bytes
		};
		snapshots = [snap, ...snapshots].sort((a, b) => b.created_at.localeCompare(a.created_at));
		remoteSnapshots = remoteSnapshots.map((s) =>
			s.id === remote.id ? { ...s, imported: true } : s
		);
		return ok(snap);
	}),

	http.get('/api/v1/snapshots/config', async () => {
		await delay(LATENCY);
		return ok(backupConfig);
	}),

	http.put('/api/v1/snapshots/config', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as Partial<BackupConfig>;
		backupConfig = { ...backupConfig, ...body };
		return ok(backupConfig);
	})
];
