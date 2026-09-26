/**
 * MSW request handlers for all /api/* endpoints.
 * Every handler returns the ApiResponse<T> envelope the real server uses:
 *   { success: true, data: T, error: null }
 */

import { http, HttpResponse, ws, delay } from 'msw';

// WebSocket links must be declared at module scope so MSW can track clients
// across the service worker boundary via a stable BroadcastChannel.
const appLogStream = ws.link('ws://*/api/apps/*/logs/stream');
const systemLogStream = ws.link('ws://*/api/system/logs/stream');
const allDatabaseLogStream = ws.link('ws://*/api/databases/logs/stream');
const databaseLogStream = ws.link('ws://*/api/databases/*/logs/stream');
const cronRunStream = ws.link('ws://*/api/cron/*/runs/stream');
const terminalStream = ws.link('ws://*/api/terminal/ws*');

import {
	MOCK_USER,
	MOCK_USERS,
	MOCK_APPS,
	MOCK_PROJECTS,
	MOCK_TEMPLATES,
	MOCK_SETTINGS,
	MOCK_ONBOARDING_STATUS,
	MOCK_DEPLOYMENTS,
	MOCK_GENERATED_DOCKERFILES,
	MOCK_DEPLOY_KEYS,
	MOCK_PREVIEWS,
	MOCK_PROJECT_PREVIEWS,
	MOCK_AUDIT_LOG,
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
	makeMockSystemMetrics,
	makeMockSystemSelf,
	makeMockAppMetrics
} from './fixtures';
import type {
	AlertRule,
	AlertIntegrationLink,
	App,
	BackupConfig,
	CronJob,
	CronJobRun,
	Deployment,
	GitHubApp,
	GithubPR,
	Integration,
	ManagedDatabase,
	TemplateCatalogEntry,
	Volume,
	VolumeInput,
	CronVolume,
	CronVolumeInput,
	PreviewDeployment,
	Project,
	ProjectPreview,
	RegistryCredential,
	Snapshot,
	StorageProvider
} from '$lib/api';
import { MOCK_ALERT_RULES } from './fixtures';

// Simulated network latency (ms). Set to 0 for instant responses.
const LATENCY = 200;

function ok<T>(data: T) {
	return HttpResponse.json({ success: true, data, error: null });
}

function err(message: string, status = 400) {
	return HttpResponse.json({ success: false, data: null, error: message }, { status });
}

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

/** Public connection string (only when public access is on). */
function mockPublicUrl(db: ManagedDatabase): string | null {
	if (!db.public_access || !db.host_port) return null;
	const isRedisLike = db.db_type === 'redis' || db.db_type === 'keydb';
	return isRedisLike
		? `redis://${db.db_user}:${db.db_password}@tond.example.com:${db.host_port}`
		: `${db.db_type}://${db.db_user}:${db.db_password}@tond.example.com:${db.host_port}/${db.db_name}`;
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
		failure_count: 0
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
const MOCK_STEP_UP_PASSWORD = 'ValidPass123!';
// ticket → target, consumed by the terminal WebSocket mock
const terminalTickets = new Map<string, { kind: string; id: string }>();
let apps: App[] = structuredClone(MOCK_APPS);
let projects: Project[] = structuredClone(MOCK_PROJECTS);
const templates: TemplateCatalogEntry[] = structuredClone(MOCK_TEMPLATES);
let alertRules: AlertRule[] = structuredClone(MOCK_ALERT_RULES);
const deployments: Record<string, Deployment[]> = structuredClone(MOCK_DEPLOYMENTS);

// Deploys started in the mock walk through the build queue on a clock, so the
// dashboard's queue position and progress can be seen:
// queued → building → deploying → succeeded. Deploys of one app run one at a
// time; a newer request supersedes one that is still queued.
const QUEUE_MS = 2500;
const BUILD_MS = 6000;
const CUTOVER_MS = 2500;
const TOTAL_MS = QUEUE_MS + BUILD_MS + CUTOVER_MS;
/** deployment id → when it may start (its queue wait ends) */
const deployClock = new Map<string, number>();
const UNFINISHED = ['queued', 'building', 'deploying'];

function advanceDeployments(appId: string) {
	const now = Date.now();
	const list = deployments[appId] ?? [];
	const queuedIds = list
		.filter((d) => d.status === 'queued')
		.map((d) => d.id)
		.reverse();
	deployments[appId] = list.map((d) => {
		const start = deployClock.get(d.id);
		if (start === undefined || !UNFINISHED.includes(d.status)) return d;
		const age = now - start;
		if (age < QUEUE_MS) {
			return { ...d, status: 'queued', queue_position: queuedIds.indexOf(d.id) + 1 };
		}
		if (age < QUEUE_MS + BUILD_MS) {
			return {
				...d,
				status: 'building',
				queue_position: null,
				started_at: new Date(start + QUEUE_MS).toISOString()
			};
		}
		if (age < TOTAL_MS) return { ...d, status: 'deploying', queue_position: null };
		deployClock.delete(d.id);
		return {
			...d,
			status: 'succeeded',
			queue_position: null,
			finished_at: new Date(start + TOTAL_MS).toISOString(),
			image_tag: d.image_tag ?? `tond-${appId}:${d.id.slice(-12)}`
		};
	});
}

function enqueueMockDeployment(appId: string, kind: string, imageTag: string | null = null) {
	advanceDeployments(appId);
	const list = deployments[appId] ?? [];
	const id = `dep_${crypto.randomUUID().replaceAll('-', '').slice(0, 16)}`;
	const now = Date.now();
	// Supersede a deploy that has not started yet.
	const waiting = list.find((d) => d.status === 'queued');
	const running = list.find((d) => d.status === 'building' || d.status === 'deploying');
	const runningStart = running ? (deployClock.get(running.id) ?? now) : now;
	if (waiting) deployClock.delete(waiting.id);
	const startAt = running ? runningStart + TOTAL_MS - QUEUE_MS : now;
	deployClock.set(id, startAt);
	const created = new Date(now).toISOString();
	const deployment: Deployment = {
		id,
		app_id: appId,
		image_tag: imageTag,
		status: 'queued',
		trigger_source: kind === 'rollback' ? 'rollback' : 'manual',
		kind,
		error: null,
		superseded_by: null,
		queue_position: running ? 2 : 1,
		has_generated_dockerfile: false,
		started_at: created,
		finished_at: null,
		created_at: created,
		image_pruned: false,
		is_current: false,
		rollback_available: false,
		rollback_unavailable_reason: 'Only successful deployments can be restored'
	};
	deployments[appId] = [
		deployment,
		...list.map((d) =>
			d.id === waiting?.id
				? {
						...d,
						status: 'superseded' as const,
						superseded_by: id,
						finished_at: created,
						queue_position: null,
						error: 'Replaced by a newer deploy before it started'
					}
				: d
		)
	];
	return {
		message:
			deployment.queue_position! > 1
				? `Deployment queued (position ${deployment.queue_position})`
				: 'Deployment queued',
		app_id: appId,
		deployment_id: id,
		status: 'queued' as const,
		queue_position: deployment.queue_position!,
		superseded: waiting?.id ?? null
	};
}
let previews: Record<string, PreviewDeployment[]> = structuredClone(MOCK_PREVIEWS);
let projectPreviews: Record<string, ProjectPreview[]> = structuredClone(MOCK_PROJECT_PREVIEWS);
let databases: ManagedDatabase[] = structuredClone(MOCK_DATABASES);
let cronJobs: CronJob[] = structuredClone(MOCK_CRON_JOBS);
const cronRuns: Record<string, CronJobRun[]> = structuredClone(MOCK_CRON_RUNS);
let registryCredentials: RegistryCredential[] = structuredClone(MOCK_REGISTRY_CREDENTIALS);
let githubApps: GitHubApp[] = structuredClone(MOCK_GITHUB_APPS);

// Per-app webhook secrets (the real server generates one at app creation).
const webhookSecrets: Record<string, string> = {};

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
		secret: webhookSecrets[app.id],
		auto_deploy: app.auto_deploy,
		watch_branch: app.watch_branch
	};
}
let users = structuredClone(MOCK_USERS);
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

function maskedEntries(vars: Record<string, string>) {
	return Object.keys(vars)
		.sort()
		.map((key) => ({ key, masked: vars[key] ? '••••••••' : '' }));
}

function appEnvResponse(app: App) {
	return {
		vars: maskedEntries(envStore(app.id)),
		preview: {
			enabled: app.preview_enabled,
			excluded_keys: app.preview_excluded_keys,
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

// alert_id → AlertIntegrationLink[]
let alertIntegrationLinks: Record<string, AlertIntegrationLink[]> = {
	alert_1: [
		{
			id: 'link_1',
			rule_id: 'alert_1',
			integration_id: 'int_telegram_1',
			integration_name: 'Ops Telegram',
			integration_kind: 'telegram',
			created_at: '2025-03-06T00:00:00.000Z'
		}
	]
};

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

export const handlers = [
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

	http.get('/api/auth/users', async () => {
		await delay(LATENCY);
		return ok(users);
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

	http.post('/api/auth/users', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as { email: string; role?: string };
		const newUser = {
			id: `usr_${Date.now()}`,
			email: body.email,
			role: body.role ?? 'member',
			totp_enabled: false,
			created_at: new Date().toISOString()
		};
		users = [...users, newUser];
		return ok(newUser);
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
		if ((body.new_password?.length ?? 0) < 8) {
			return err('New password must be at least 8 characters', 400);
		}
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
				expires_at: new Date(Date.now() + 82800_000).toISOString(),
				is_current: true
			},
			{
				id: 'sess_other',
				user_agent: 'tond-cli/0.1.0',
				ip_address: '192.168.1.42',
				created_at: new Date(Date.now() - 86400_000).toISOString(),
				last_used_at: new Date(Date.now() - 7200_000).toISOString(),
				expires_at: new Date(Date.now() + 3600_000).toISOString(),
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
		return ok(templates);
	}),

	http.post('/api/projects', async ({ request }) => {
		await delay(LATENCY * 2);
		const body = (await request.json()) as {
			name: string;
			description?: string;
			env_vars?: Record<string, string>;
		};
		const newProject: Project = {
			id: `proj_${Date.now()}`,
			name: body.name,
			description: body.description ?? null,
			env_vars: body.env_vars ?? {},
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
		const body = (await request.json()) as Partial<
			Pick<Project, 'name' | 'description' | 'env_vars'>
		>;
		projects[idx] = { ...projects[idx], ...body, updated_at: new Date().toISOString() };
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
			preview_excluded_keys: [],
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
		return ok(app);
	}),

	http.delete('/api/apps/:id', async ({ params }) => {
		await delay(LATENCY);
		apps = apps.filter((a) => a.id !== params.id);
		return ok(null);
	}),

	http.post('/api/apps/:id/deploy', async ({ params }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		apps[idx] = { ...apps[idx], updated_at: new Date().toISOString() };
		return HttpResponse.json(
			{ success: true, data: enqueueMockDeployment(params.id as string, 'deploy'), error: null },
			{ status: 202 }
		);
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
			exclude?: string[];
			include?: string[];
		};
		const hasExclusions = (body.exclude?.length ?? 0) + (body.include?.length ?? 0) > 0;
		const problem = hasExclusions && !body.set && !body.unset ? null : validateEnvPatch(body);
		if (problem) return err(problem);
		const app = apps[idx];
		previewEnvVars[app.id] = applyEnvPatch(previewStore(app.id), body);
		const excluded = new Set(app.preview_excluded_keys);
		for (const key of body.exclude ?? []) excluded.add(key);
		for (const key of body.include ?? []) excluded.delete(key);
		apps[idx] = { ...app, preview_excluded_keys: [...excluded].sort() };
		return ok(appEnvResponse(apps[idx]));
	}),

	http.put('/api/apps/:id/domain', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx === -1) return err('App not found', 404);
		const body = (await request.json()) as { domain: string };
		apps[idx] = { ...apps[idx], domain: body.domain, updated_at: new Date().toISOString() };
		queueMockCertificate(body.domain);
		return ok(apps[idx]);
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

	http.get('/api/apps/:id/deployments', async ({ params }) => {
		advanceDeployments(params.id as string);
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
		const idx = apps.findIndex((a) => a.id === params.id);
		if (idx !== -1) {
			apps[idx] = { ...apps[idx], status: 'running', updated_at: new Date().toISOString() };
		}
		// The restored image becomes current.
		deployments[params.id as string] = list.map((d) => ({
			...d,
			is_current: d.id === target.id,
			rollback_available: d.id !== target.id && d.status === 'succeeded' && !d.image_pruned,
			rollback_unavailable_reason:
				d.id === target.id ? 'This image is already running' : d.rollback_unavailable_reason
		}));
		const queued = enqueueMockDeployment(params.id as string, 'rollback', target.image_tag);
		return HttpResponse.json(
			{
				success: true,
				data: {
					...queued,
					message: 'Rollback queued',
					image_tag: target.image_tag,
					target_deployment_id: target.id
				},
				error: null
			},
			{ status: 202 }
		);
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

	http.post('/api/apps/:id/webhook-secret/rotate', async ({ params }) => {
		await delay(LATENCY);
		const app = apps.find((a) => a.id === params.id || a.name === params.id);
		if (!app) return err('App not found', 404);
		webhookSecrets[app.id] = randomHex(32);
		return ok(webhookConfig(app));
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

	http.get('/api/apps/:id/metrics', async ({ request }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const period = url.searchParams.get('period') ?? '1h';
		return ok(makeMockAppMetrics(period));
	}),

	http.get('/api/system/metrics', async () => {
		await delay(LATENCY);
		return ok(makeMockSystemMetrics());
	}),

	http.get('/api/system/self', async () => {
		await delay(LATENCY);
		const self = makeMockSystemSelf();
		const queued = Object.values(deployments)
			.flat()
			.filter((d) => d.status === 'queued').length;
		const active = Object.values(deployments)
			.flat()
			.filter((d) => d.status === 'building' || d.status === 'deploying').length;
		return ok({ ...self, build_queue: { ...self.build_queue, queued, active } });
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

	http.put('/api/settings', async ({ request }) => {
		await delay(LATENCY);
		const body = (await request.json()) as Partial<typeof MOCK_SETTINGS>;
		if (body.dashboard_domain !== undefined) {
			body.dashboard_domain = normalizeDomain(body.dashboard_domain);
		}
		if (body.preview_domain !== undefined) {
			body.preview_domain = normalizeDomain(body.preview_domain);
		}
		const previewError = mockPreviewDomainError(
			body.preview_domain ?? MOCK_SETTINGS.preview_domain,
			body.dashboard_domain ?? MOCK_SETTINGS.dashboard_domain
		);
		if (previewError) return err(previewError);
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

	// ── Alert Rules ───────────────────────────────────────────────────────────

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
		const body = (await request.json()) as Omit<
			AlertRule,
			'id' | 'enabled' | 'last_fired_at' | 'created_at' | 'updated_at'
		>;
		const validMetrics = ['cpu', 'memory', 'app_down', 'disk_usage'];
		const validEvents = [
			'deployment_success',
			'deployment_failure',
			'cron_success',
			'cron_failure'
		];
		if (body.metric && !validMetrics.includes(body.metric)) {
			return err(`metric must be one of: ${validMetrics.join(', ')}`, 400);
		}
		if (body.events) {
			for (const ev of body.events) {
				if (!validEvents.includes(ev)) {
					return err(`event must be one of: ${validEvents.join(', ')}`, 400);
				}
			}
		}
		if (!body.metric && (!body.events || body.events.length === 0)) {
			return err('At least one metric or event is required', 400);
		}
		const newRule: AlertRule = {
			id: `alert_${Date.now()}`,
			app_id: body.app_id ?? null,
			name: body.name,
			metric: body.metric ?? null,
			threshold: body.threshold ?? 0,
			webhook_url: body.webhook_url,
			enabled: true,
			events: body.events ?? [],
			last_fired_at: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString()
		};
		alertRules = [...alertRules, newRule];
		return HttpResponse.json({ success: true, data: newRule, error: null }, { status: 201 });
	}),

	http.patch('/api/alerts/:id', async ({ params, request }) => {
		await delay(LATENCY);
		const idx = alertRules.findIndex((r) => r.id === params.id);
		if (idx === -1) return err('Alert rule not found', 404);
		const body = (await request.json()) as Partial<
			Pick<AlertRule, 'name' | 'metric' | 'threshold' | 'webhook_url' | 'enabled' | 'events'>
		>;
		if (body.metric) {
			const validMetrics = ['cpu', 'memory', 'app_down', 'disk_usage'];
			if (!validMetrics.includes(body.metric)) {
				return err(`metric must be one of: ${validMetrics.join(', ')}`, 400);
			}
		}
		if (body.events) {
			const validEvents = [
				'deployment_success',
				'deployment_failure',
				'cron_success',
				'cron_failure'
			];
			for (const ev of body.events) {
				if (!validEvents.includes(ev)) {
					return err(`event must be one of: ${validEvents.join(', ')}`, 400);
				}
			}
		}
		alertRules[idx] = { ...alertRules[idx], ...body, updated_at: new Date().toISOString() };
		return ok(alertRules[idx]);
	}),

	http.delete('/api/alerts/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = alertRules.some((r) => r.id === params.id);
		if (!exists) return err('Alert rule not found', 404);
		alertRules = alertRules.filter((r) => r.id !== params.id);
		return ok({ deleted: params.id });
	}),

	// ── Audit Log ─────────────────────────────────────────────────────────────

	http.get('/api/audit-log', async ({ request }) => {
		await delay(LATENCY);
		const url = new URL(request.url);
		const appId = url.searchParams.get('app_id');
		const limit = parseInt(url.searchParams.get('limit') ?? '50', 10);
		let entries = MOCK_AUDIT_LOG;
		if (appId) entries = entries.filter((e) => e.app_id === appId);
		return ok(entries.slice(0, limit));
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
			db_password: 'mock_password_new',
			root_password: isMysqlLike ? 'mock_root_password_new' : null,
			memory_limit: body.memory_limit ?? null,
			custom_external_url: null,
			custom_internal_url: null,
			image_used: defaultImages[body.db_type] ?? `${body.db_type}:latest`,
			custom_image: null,
			created_at: new Date().toISOString(),
			updated_at: new Date().toISOString(),
			public_access: body.public_access ?? false
		};
		databases = [...databases, newDb];
		const newDbUser = newDb.db_user!;
		const newDbPass = newDb.db_password!;
		const newConnectionString = isRedisLike
			? `redis://${newDbUser}:${newDbPass}@localhost:${newDb.host_port}`
			: `${body.db_type}://${newDbUser}:${newDbPass}@localhost:${newDb.host_port}/${newDb.db_name}`;
		return ok({
			...newDb,
			connection_string: newConnectionString,
			public_connection_string: mockPublicUrl(newDb),
			internal_connection_string: null
		});
	}),

	http.get('/api/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		const db = databases.find((d) => d.id === params.id || d.name === params.id);
		if (!db) return err('Database not found', 404);
		const isRedisLike = db.db_type === 'redis' || db.db_type === 'keydb';
		const redisPort = 6379;
		const computedExternal = db.host_port
			? isRedisLike
				? db.db_user && db.db_password
					? `redis://${db.db_user}:${db.db_password}@localhost:${db.host_port}`
					: `redis://localhost:${db.host_port}`
				: `${db.db_type}://${db.db_user}:${db.db_password}@localhost:${db.host_port}/${db.db_name}`
			: null;
		const computedInternal = isRedisLike
			? db.db_user && db.db_password
				? `redis://${db.db_user}:${db.db_password}@tond-db-${db.name}:${redisPort}`
				: `redis://tond-db-${db.name}:${redisPort}`
			: `${db.db_type}://${db.db_user}:${db.db_password}@tond-db-${db.name}:${db.db_type === 'mysql' || db.db_type === 'mariadb' ? 3306 : 5432}/${db.db_name}`;
		return ok({
			...db,
			connection_string: db.custom_external_url ?? computedExternal,
			public_connection_string: mockPublicUrl(db),
			internal_connection_string: db.custom_internal_url ?? computedInternal
		});
	}),

	http.delete('/api/databases/:id', async ({ params }) => {
		await delay(LATENCY);
		databases = databases.filter((d) => d.id !== params.id && d.name !== params.id);
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
		if (body.custom_external_url !== undefined) {
			updated.custom_external_url =
				body.custom_external_url === '' || body.custom_external_url === null
					? null
					: body.custom_external_url;
		}
		if (body.custom_internal_url !== undefined) {
			updated.custom_internal_url =
				body.custom_internal_url === '' || body.custom_internal_url === null
					? null
					: body.custom_internal_url;
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
		const db = databases[idx];
		// Return ManagedDatabaseDetail with resolved connection strings
		const isRedisLike2 = db.db_type === 'redis' || db.db_type === 'keydb';
		const redisPort2 = 6379;
		const computedExternal = db.host_port
			? isRedisLike2
				? db.db_user && db.db_password
					? `redis://${db.db_user}:${db.db_password}@localhost:${db.host_port}`
					: `redis://localhost:${db.host_port}`
				: `${db.db_type}://${db.db_user}:${db.db_password}@localhost:${db.host_port}/${db.db_name}`
			: null;
		const computedInternal = isRedisLike2
			? db.db_user && db.db_password
				? `redis://${db.db_user}:${db.db_password}@tond-db-${db.name}:${redisPort2}`
				: `redis://tond-db-${db.name}:${redisPort2}`
			: `${db.db_type}://${db.db_user}:${db.db_password}@tond-db-${db.name}:${db.db_type === 'mysql' || db.db_type === 'mariadb' ? 3306 : 5432}/${db.db_name}`;
		return ok({
			...db,
			connection_string: db.custom_external_url ?? computedExternal,
			public_connection_string: mockPublicUrl(db),
			internal_connection_string: db.custom_internal_url ?? computedInternal
		});
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
		const renamed = (next: string | null | undefined, now: string | null) =>
			!!next?.trim() && next.trim() !== now;
		if (renamed(body.db_name, current.db_name) || renamed(body.db_user, current.db_user)) {
			return err(
				"Invalid input: The username of an existing database can't be changed; only passwords can."
			);
		}
		const changed: string[] = [];
		const updated = { ...current };
		const nextPw = body.db_password?.trim();
		if (nextPw && nextPw !== current.db_password) {
			updated.db_password = nextPw;
			changed.push('db_password');
		}
		const nextRoot = body.root_password?.trim();
		if (nextRoot && nextRoot !== current.root_password) {
			updated.root_password = nextRoot;
			changed.push('root_password');
		}
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
		updated.updated_at = new Date().toISOString();
		databases[idx] = updated;
		const db = databases[idx];
		const isRedisLikeC = db.db_type === 'redis' || db.db_type === 'keydb';
		const redisPortC = 6379;
		const computedExternalC = db.host_port
			? isRedisLikeC
				? db.db_user && db.db_password
					? `redis://${db.db_user}:${db.db_password}@localhost:${db.host_port}`
					: `redis://localhost:${db.host_port}`
				: `${db.db_type}://${db.db_user}:${db.db_password}@localhost:${db.host_port}/${db.db_name}`
			: null;
		const computedInternalC = isRedisLikeC
			? db.db_user && db.db_password
				? `redis://${db.db_user}:${db.db_password}@tond-db-${db.name}:${redisPortC}`
				: `redis://tond-db-${db.name}:${redisPortC}`
			: `${db.db_type}://${db.db_user}:${db.db_password}@tond-db-${db.name}:${db.db_type === 'mysql' || db.db_type === 'mariadb' ? 3306 : 5432}/${db.db_name}`;
		return ok({
			database: {
				...db,
				connection_string: db.custom_external_url ?? computedExternalC,
				public_connection_string: mockPublicUrl(db),
				internal_connection_string: db.custom_internal_url ?? computedInternalC
			},
			changed
		});
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
		const isRedisLike = updated.db_type === 'redis' || updated.db_type === 'keydb';
		const redisPort = 6379;
		const computedExternal = updated.host_port
			? isRedisLike
				? updated.db_user && updated.db_password
					? `redis://${updated.db_user}:${updated.db_password}@localhost:${updated.host_port}`
					: `redis://localhost:${updated.host_port}`
				: `${updated.db_type}://${updated.db_user}:${updated.db_password}@localhost:${updated.host_port}/${updated.db_name}`
			: null;
		const computedInternal = isRedisLike
			? updated.db_user && updated.db_password
				? `redis://${updated.db_user}:${updated.db_password}@tond-db-${updated.name}:${redisPort}`
				: `redis://tond-db-${updated.name}:${redisPort}`
			: `${updated.db_type}://${updated.db_user}:${updated.db_password}@tond-db-${updated.name}:${updated.db_type === 'mysql' || updated.db_type === 'mariadb' ? 3306 : 5432}/${updated.db_name}`;
		return ok({
			...updated,
			connection_string: updated.custom_external_url ?? computedExternal,
			public_connection_string: mockPublicUrl(updated),
			internal_connection_string: updated.custom_internal_url ?? computedInternal
		});
	}),

	// ── Cron Jobs (Phase 6.4) ──────────────────────────────────────────────────

	http.get('/api/cron', async () => {
		await delay(LATENCY);
		return ok(cronJobs);
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
			env: body.env && Object.keys(body.env).length > 0 ? body.env : null,
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
			concurrency_policy: string;
			memory_mb: number | null;
			cpu_shares: number | null;
			timezone: string | null;
			max_retries: number;
			retry_delay_seconds: number;
			volumes: CronVolumeInput[] | null;
		}>;
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
			...('env' in body && {
				env: body.env && Object.keys(body.env).length > 0 ? body.env : null
			}),
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
		const body = (await request.json()) as { name: string; kind: string; config: unknown };
		const newIntegration: Integration = {
			id: `int_${Date.now()}`,
			name: body.name,
			kind: body.kind as Integration['kind'],
			config: body.config as Integration['config'],
			enabled: true,
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
		const body = (await request.json()) as Partial<
			Pick<Integration, 'name' | 'config' | 'enabled'>
		>;
		integrations[idx] = { ...integrations[idx], ...body, updated_at: new Date().toISOString() };
		return ok(integrations[idx]);
	}),

	http.delete('/api/integrations/:id', async ({ params }) => {
		await delay(LATENCY);
		const exists = integrations.some((i) => i.id === params.id);
		if (!exists) return err('Integration not found', 404);
		integrations = integrations.filter((i) => i.id !== params.id);
		// Remove any alert links referencing this integration
		for (const ruleId of Object.keys(alertIntegrationLinks)) {
			alertIntegrationLinks[ruleId] = alertIntegrationLinks[ruleId].filter(
				(l) => l.integration_id !== params.id
			);
		}
		return ok({ deleted: params.id });
	}),

	http.post('/api/integrations/:id/test', async ({ params }) => {
		await delay(LATENCY * 3);
		const integration = integrations.find((i) => i.id === params.id);
		if (!integration) return err('Integration not found', 404);
		if (!integration.enabled) return err('Integration is disabled', 400);
		return ok({ message: `Test message sent via ${integration.kind}` });
	}),

	// ── Alert ↔ Integration Links ──────────────────────────────────────────────

	http.get('/api/alerts/:id/integrations', async ({ params }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		return ok(alertIntegrationLinks[params.id as string] ?? []);
	}),

	http.post('/api/alerts/:id/integrations', async ({ params, request }) => {
		await delay(LATENCY);
		const rule = alertRules.find((r) => r.id === params.id);
		if (!rule) return err('Alert rule not found', 404);
		const body = (await request.json()) as { integration_id: string };
		const integration = integrations.find((i) => i.id === body.integration_id);
		if (!integration) return err('Integration not found', 404);
		const ruleId = params.id as string;
		const existing = alertIntegrationLinks[ruleId] ?? [];
		if (existing.some((l) => l.integration_id === body.integration_id)) {
			return err('Integration already linked to this rule', 409);
		}
		const link: AlertIntegrationLink = {
			id: `link_${Date.now()}`,
			rule_id: ruleId,
			integration_id: body.integration_id,
			integration_name: integration.name,
			integration_kind: integration.kind,
			created_at: new Date().toISOString()
		};
		alertIntegrationLinks = {
			...alertIntegrationLinks,
			[ruleId]: [...existing, link]
		};
		return HttpResponse.json({ success: true, data: link, error: null }, { status: 201 });
	}),

	http.delete('/api/alerts/:id/integrations/:integrationId', async ({ params }) => {
		await delay(LATENCY);
		const ruleId = params.id as string;
		const rule = alertRules.find((r) => r.id === ruleId);
		if (!rule) return err('Alert rule not found', 404);
		const links = alertIntegrationLinks[ruleId] ?? [];
		const exists = links.some((l) => l.integration_id === params.integrationId);
		if (!exists) return err('Integration link not found', 404);
		alertIntegrationLinks = {
			...alertIntegrationLinks,
			[ruleId]: links.filter((l) => l.integration_id !== params.integrationId)
		};
		return ok(null);
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
			notes: null
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
			notes: null
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
		const body = (await request.json()) as any;
		return ok({ message: `Database '${body.db_name}' restored successfully.` });
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
