/**
 * Shared fixture data used by MSW mock handlers.
 * All IDs are stable so navigation between pages works (e.g. /apps/:id links).
 */

import type {
	App,
	AlertRule,
	AuditLogEntry,
	BackupConfig,
	Certificate,
	CronJob,
	CronJobRun,
	Deployment,
	DeploymentLogLine,
	DeployPhase,
	PhaseRecord,
	DeployKey,
	GitHubApp,
	GitHubRepo,
	GitHubBranch,
	Integration,
	ManagedDatabase,
	TemplateCatalogEntry,
	MetricsRange,
	MetricsResponse,
	OnboardingStatus,
	PreviewDeployment,
	Project,
	ProjectPreview,
	RegistryCredential,
	ServerSettings,
	Snapshot,
	StorageProvider,
	SystemMetrics,
	User,
	AdminUser,
	ApiToken,
	TokenCatalog
} from '$lib/api';
import { ALL_SCOPES } from '$lib/access';

// ── Users ──────────────────────────────────────────────────────────────────

export const MOCK_USER: User = {
	id: 'usr_dev',
	email: 'admin@localhost',
	role: 'admin',
	totp_enabled: false,
	created_at: '2025-01-01T00:00:00.000Z',
	disabled: false,
	password_set: true,
	last_login_at: new Date(Date.now() - 20 * 60_000).toISOString(),
	scopes: ALL_SCOPES
};

const DEPLOYER_SCOPES = [
	'apps:read',
	'apps:deploy',
	'apps:control',
	'apps:rollback',
	'env:read-names',
	'env:write',
	'logs:read',
	'metrics:read',
	'events:read',
	'cron:read',
	'cron:trigger',
	'previews:write',
	'db:read',
	'snapshots:read',
	'snapshots:create'
];
const VIEWER_SCOPES = [
	'apps:read',
	'env:read-names',
	'db:read',
	'snapshots:read',
	'logs:read',
	'metrics:read',
	'events:read',
	'cron:read'
];

export const MOCK_USERS: AdminUser[] = [
	{ ...MOCK_USER, active_sessions: 2, is_you: true },
	{
		id: 'usr_2',
		email: 'dana@example.com',
		role: 'deployer',
		totp_enabled: true,
		created_at: '2025-02-01T00:00:00.000Z',
		disabled: false,
		password_set: true,
		last_login_at: new Date(Date.now() - 3 * 3600_000).toISOString(),
		scopes: DEPLOYER_SCOPES,
		active_sessions: 1,
		is_you: false
	},
	{
		id: 'usr_3',
		email: 'victor@example.com',
		role: 'viewer',
		totp_enabled: false,
		created_at: '2025-03-10T00:00:00.000Z',
		disabled: false,
		password_set: false,
		last_login_at: null,
		scopes: VIEWER_SCOPES,
		active_sessions: 0,
		is_you: false
	},
	{
		id: 'usr_4',
		email: 'former@example.com',
		role: 'deployer',
		totp_enabled: false,
		created_at: '2024-11-01T00:00:00.000Z',
		disabled: true,
		password_set: true,
		last_login_at: '2025-01-20T10:00:00.000Z',
		scopes: DEPLOYER_SCOPES,
		active_sessions: 0,
		is_you: false
	}
];

// ── API tokens ─────────────────────────────────────────────────────────────

export const MOCK_TOKEN_CATALOG: TokenCatalog = {
	groups: [
		{
			id: 'apps',
			label: 'Apps',
			scopes: [
				{
					id: 'apps:read',
					description: 'See apps, their settings, domains and deployment history'
				},
				{ id: 'apps:deploy', description: 'Deploy apps (build and release a new version)' },
				{
					id: 'apps:write',
					description: 'Create apps and change their configuration, domains and volumes'
				},
				{ id: 'apps:control', description: 'Start, stop and restart apps' },
				{ id: 'apps:rollback', description: 'Roll an app back to an earlier deployment' },
				{ id: 'apps:delete', description: 'Delete apps and projects' }
			]
		},
		{
			id: 'secrets',
			label: 'Secrets',
			scopes: [
				{
					id: 'env:read-names',
					description: 'See the names of environment variables (not their values)'
				},
				{ id: 'env:read-values', description: 'Reveal environment variable values' },
				{ id: 'env:write', description: 'Add, change and remove environment variables' },
				{
					id: 'db:credentials',
					description: 'Reveal and change database passwords and connection strings'
				}
			]
		},
		{
			id: 'data',
			label: 'Data',
			scopes: [
				{ id: 'db:read', description: 'See databases and their status' },
				{ id: 'db:write', description: 'Create, start, stop and reconfigure databases' },
				{ id: 'db:delete', description: 'Delete databases' },
				{ id: 'snapshots:read', description: 'See snapshots and backup status' },
				{ id: 'snapshots:create', description: 'Take snapshots' },
				{
					id: 'snapshots:restore',
					description: 'Restore and delete snapshots (overwrites live data)'
				}
			]
		},
		{
			id: 'ops',
			label: 'Ops',
			scopes: [
				{ id: 'logs:read', description: 'Read build, app, database and cron logs' },
				{ id: 'metrics:read', description: 'See CPU, memory and network metrics and alert rules' },
				{ id: 'events:read', description: 'See the activity (audit) log' },
				{ id: 'cron:read', description: 'See cron jobs and their runs' },
				{ id: 'cron:write', description: 'Create, change and delete cron jobs' },
				{ id: 'cron:trigger', description: 'Run a cron job now' },
				{ id: 'previews:write', description: 'Create and delete preview deployments' }
			]
		},
		{
			id: 'admin',
			label: 'Admin',
			scopes: [
				{
					id: 'settings:write',
					description: 'Change server settings, integrations, alerts and backups'
				},
				{ id: 'users:admin', description: 'Invite, change and remove users and their sessions' },
				{ id: 'tokens:admin', description: "See and revoke every user's API tokens" },
				{ id: 'terminal:host', description: 'Open terminal sessions (host shell and containers)' }
			]
		}
	],
	presets: [
		{
			id: 'readonly',
			label: 'Read-only',
			description: "Look but don't touch: apps, logs, metrics, activity. No secret values.",
			scopes: VIEWER_SCOPES
		},
		{
			id: 'deployer',
			label: 'Deployer (CI)',
			description: 'Deploy and read one app from CI. Pair it with an app filter.',
			scopes: ['apps:read', 'apps:deploy', 'logs:read']
		},
		{
			id: 'agent-operator',
			label: 'Agent operator',
			description: 'For AI agents: read, deploy, restart, roll back, previews and env var names.',
			scopes: [
				'apps:read',
				'apps:deploy',
				'apps:control',
				'apps:rollback',
				'previews:write',
				'env:read-names',
				'logs:read',
				'metrics:read',
				'events:read'
			]
		}
	],
	roles: [
		{ id: 'admin', description: 'Full control', scopes: ALL_SCOPES },
		{ id: 'deployer', description: 'Ships code', scopes: DEPLOYER_SCOPES },
		{ id: 'viewer', description: 'Read-only', scopes: VIEWER_SCOPES }
	],
	grantable: ALL_SCOPES
};

export const MOCK_TOKENS: ApiToken[] = [
	{
		id: 'tok_ci',
		name: 'github-actions-web',
		prefix: 'tond_pat_3f9a2c1d',
		kind: 'ci',
		scopes: ['apps:read', 'apps:deploy', 'logs:read'],
		app_ids: ['app_running'],
		project_ids: [],
		allowed_ips: [],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: new Date(Date.now() + 60 * 86400_000).toISOString(),
		last_used_at: new Date(Date.now() - 42 * 60_000).toISOString(),
		last_used_ip: '140.82.112.4',
		revoked_at: null,
		created_at: '2025-02-10T09:00:00.000Z'
	},
	{
		id: 'tok_agent',
		name: 'claude-ops-agent',
		prefix: 'tond_pat_b71e0d44',
		kind: 'agent',
		scopes: [
			'apps:read',
			'apps:deploy',
			'apps:control',
			'apps:rollback',
			'previews:write',
			'env:read-names',
			'logs:read',
			'metrics:read',
			'events:read'
		],
		app_ids: [],
		project_ids: ['proj_1'],
		allowed_ips: ['203.0.113.0/24'],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: new Date(Date.now() + 5 * 3600_000).toISOString(),
		last_used_at: new Date(Date.now() - 3 * 60_000).toISOString(),
		last_used_ip: '203.0.113.20',
		revoked_at: null,
		created_at: new Date(Date.now() - 19 * 3600_000).toISOString()
	},
	{
		id: 'tok_old',
		name: 'laptop-scripts',
		prefix: 'tond_pat_0c55e9a1',
		kind: 'personal',
		scopes: ['apps:read', 'logs:read'],
		app_ids: [],
		project_ids: [],
		allowed_ips: [],
		owner_id: 'usr_dev',
		owner_label: 'admin@localhost',
		expires_at: null,
		last_used_at: '2025-01-02T10:00:00.000Z',
		last_used_ip: '192.168.1.42',
		revoked_at: '2025-02-01T12:00:00.000Z',
		created_at: '2024-12-01T09:00:00.000Z'
	}
];

// ── Projects (Phase 6.1) ───────────────────────────────────────────────────

export const MOCK_PROJECTS: Project[] = [
	{
		id: 'proj_1',
		name: 'web-platform',
		description: 'Frontend and backend for the main product',
		env_vars: { NODE_ENV: 'production', LOG_LEVEL: 'info' },
		active_preview_count: 1,
		created_at: '2025-01-08T10:00:00.000Z',
		updated_at: '2025-02-20T09:00:00.000Z'
	},
	{
		id: 'proj_2',
		name: 'data-pipeline',
		description: null,
		env_vars: {},
		active_preview_count: 0,
		created_at: '2025-02-15T14:00:00.000Z',
		updated_at: '2025-02-15T14:00:00.000Z'
	}
];

export const MOCK_TEMPLATES: TemplateCatalogEntry[] = [
	{
		id: 'plausible',
		name: 'Plausible Analytics',
		description: 'Self-hosted privacy-friendly analytics',
		category: 'analytics',
		tags: ['analytics', 'privacy'],
		maintainers: ['tond'],
		project: {
			name: 'plausible',
			description: 'Plausible app and supporting services'
		},
		source: {
			kind: 'image',
			url: 'ghcr.io/plausible/community-edition',
			ref: 'latest'
		},
		deploy: {
			method: 'image',
			default_port: 8000,
			health_check_path: '/',
			volumes: [
				{
					container_path: '/var/lib/plausible',
					description: 'Persistent application data and uploaded assets'
				}
			]
		},
		required_env: [
			{
				key: 'BASE_URL',
				description: 'Public URL for the instance',
				required: true,
				value: null
			},
			{
				key: 'SECRET_KEY_BASE',
				description: 'Secret key used by Plausible',
				required: true,
				value: null
			},
			{
				key: 'DATABASE_URL',
				description: 'Primary PostgreSQL connection string',
				required: true,
				value: '${db.plausible-db.internal_connection_string}'
			}
		],
		optional_env: [],
		dependencies: [
			{
				name: 'plausible-db',
				kind: 'database',
				engine: 'postgres',
				required: true
			}
		],
		recommended_domains: ['stats.example.com'],
		post_deploy_checks: [
			'Open / to verify the dashboard loads',
			'Add your site domain inside Plausible after the first login'
		],
		setup_notes: 'Configure BASE_URL and a PostgreSQL connection before first deploy.'
	}
];

// ── App environment variables (served masked by GET /apps/:id/env) ─────────

export const MOCK_APP_ENV_VARS: Record<string, Record<string, string>> = {
	app_running: {
		DATABASE_URL: 'postgres://api:s3cret@db.internal:5432/api',
		SECRET_KEY: 'sk_live_4f9a2c7e1b',
		LOG_LEVEL: 'info',
		FEATURE_FLAGS: ''
	},
	app_stopped: {
		NEXT_PUBLIC_API_URL: 'https://api.example.com'
	}
};

export const MOCK_PREVIEW_ENV_VARS: Record<string, Record<string, string>> = {
	app_running: { DATABASE_URL: 'postgres://preview:pass@localhost/preview_db' }
};

// ── Apps ───────────────────────────────────────────────────────────────────

export const MOCK_APPS: App[] = [
	{
		id: 'app_running',
		name: 'api-service',
		domain: 'api.example.com',
		source_type: 'git',
		source_url: 'https://github.com/example/api-service',
		status: 'running',
		container_port: 8080,
		host_port: 32001,
		memory_limit: 536870912,
		cpu_limit: 1000000000,
		health_check_path: '/healthz',
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [
			{
				name: 'uploads',
				host_path: '/opt/tond/data/volumes/api-service/uploads',
				container_path: '/app/uploads'
			}
		],
		drain_timeout: 30,
		build_method: 'tbuild',
		auto_deploy: true,
		watch_branch: 'main',
		deploy_key_pub:
			'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM0mock+deploy+key+for+api+service tond@deploy',
		project_id: 'proj_1',
		registry_credential_id: null,
		github_app_id: 'ghapp_1',
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: true,
		preview_excluded_keys: ['DATABASE_URL', 'SECRET_KEY'],
		created_at: '2025-01-10T12:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z'
	},
	{
		id: 'app_stopped',
		name: 'web-frontend',
		domain: 'app.example.com',
		source_type: 'git',
		source_url: 'https://github.com/example/web-frontend',
		status: 'stopped',
		container_port: 3000,
		host_port: null,
		memory_limit: 268435456,
		cpu_limit: 500000000,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [],
		drain_timeout: 15,
		build_method: null,
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: 'proj_1',
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_excluded_keys: [],
		created_at: '2025-01-15T08:00:00.000Z',
		updated_at: '2025-02-28T14:22:00.000Z'
	},
	{
		id: 'app_failed',
		name: 'worker-queue',
		domain: null,
		source_type: 'git',
		source_url: 'https://github.com/example/worker-queue',
		status: 'failed',
		container_port: 4000,
		host_port: null,
		memory_limit: null,
		cpu_limit: null,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 4,
		volumes: [],
		drain_timeout: 30,
		build_method: 'tbuild',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: 'proj_2',
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_excluded_keys: [],
		created_at: '2025-02-20T10:00:00.000Z',
		updated_at: '2025-03-05T11:45:00.000Z'
	},
	{
		id: 'app_unhealthy',
		name: 'postgres-proxy',
		domain: 'db.example.com',
		source_type: 'image',
		source_url: null,
		status: 'unhealthy',
		container_port: 5432,
		host_port: 32004,
		memory_limit: 1073741824,
		cpu_limit: 2000000000,
		health_check_path: '/ping',
		health_check_interval: 15,
		health_failure_threshold: 2,
		health_consecutive_failures: 2,
		volumes: [
			{
				name: 'pgdata',
				host_path: '/opt/tond/data/volumes/api-server/pgdata',
				container_path: '/var/lib/postgresql/data'
			}
		],
		drain_timeout: 60,
		build_method: 'dockerfile',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: null,
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_excluded_keys: [],
		created_at: '2025-02-01T00:00:00.000Z',
		updated_at: '2025-03-07T06:00:00.000Z'
	},
	{
		id: 'app_pending',
		name: 'ml-inference',
		domain: null,
		source_type: 'git',
		source_url: 'https://github.com/example/ml-inference',
		status: 'pending',
		container_port: 7000,
		host_port: null,
		memory_limit: 2147483648,
		cpu_limit: 4000000000,
		health_check_path: null,
		health_check_interval: 30,
		health_failure_threshold: 3,
		health_consecutive_failures: 0,
		volumes: [],
		drain_timeout: 30,
		build_method: 'dockerfile',
		auto_deploy: false,
		watch_branch: null,
		deploy_key_pub: null,
		project_id: null,
		registry_credential_id: null,
		github_app_id: null,
		dockerfile_content: null,
		compose_content: null,
		base_dir: null,
		static_build: false,
		preview_enabled: false,
		preview_excluded_keys: [],
		created_at: '2025-03-08T07:00:00.000Z',
		updated_at: '2025-03-08T07:00:00.000Z'
	}
];

// ── Settings ───────────────────────────────────────────────────────────────

export const MOCK_SETTINGS: ServerSettings = {
	server_name: 'Tond Server',
	dashboard_domain: 'dash.example.com',
	preview_domain: 'example-previews.com',
	session_idle_timeout_hours: 12,
	session_max_age_days: 30
};

// ── Certificates ───────────────────────────────────────────────────────────

const minutesFromNow = (m: number) => new Date(Date.now() + m * 60_000).toISOString();

export const MOCK_CERTIFICATES: Certificate[] = [
	{
		id: 'cert-001',
		domain: 'api.example.com',
		cert_type: 'acme',
		status: 'active',
		issued_at: '2026-08-01T00:00:00Z',
		expires_at: '2026-10-30T00:00:00Z',
		last_error: null,
		last_attempt_at: '2026-08-01T00:00:00Z',
		next_retry_at: null,
		failure_count: 0
	},
	{
		id: 'cert-002',
		domain: 'app.example.com',
		cert_type: 'acme',
		status: 'pending',
		issued_at: '2026-09-20T00:00:00Z',
		expires_at: '2026-12-19T00:00:00Z',
		last_error:
			"DNS for app.example.com points to 1.2.3.4, expected 5.6.7.8. Update the domain's A/AAAA record to this server.",
		last_attempt_at: minutesFromNow(-4),
		next_retry_at: minutesFromNow(12),
		failure_count: 4
	},
	{
		id: 'cert-003',
		domain: 'dash.example.com',
		cert_type: 'acme',
		status: 'failed',
		issued_at: '2026-09-20T00:00:00Z',
		expires_at: '2026-12-19T00:00:00Z',
		last_error:
			'polling order ready: order failed: 403 urn:ietf:params:acme:error:unauthorized: Invalid response from http://dash.example.com/.well-known/acme-challenge/abc: 404',
		last_attempt_at: minutesFromNow(-1),
		next_retry_at: minutesFromNow(1),
		failure_count: 1
	}
];

export const MOCK_ONBOARDING_STATUS: OnboardingStatus = {
	dashboard_domain: MOCK_SETTINGS.dashboard_domain,
	is_fresh_install: false,
	has_projects: true,
	has_apps: true,
	has_databases: true,
	has_cron_jobs: true,
	server_public_ip: '203.0.113.10',
	domain_status: {
		configured: true,
		expected_target: '203.0.113.10',
		dns_matches_server: true,
		https_ready: false,
		certificate_status: 'pending'
	}
};

// ── System Metrics ─────────────────────────────────────────────────────────

export function makeMockSystemMetrics(): SystemMetrics {
	return {
		cpu_percent: 34.7 + Math.random() * 5,
		mem_used: 3_700_000_000 + Math.random() * 100_000_000,
		mem_total: 8_000_000_000,
		mem_percent: 46.3 + Math.random() * 2,
		disk_used: 42_000_000_000 + Math.random() * 500_000_000,
		disk_total: 160_000_000_000,
		disk_percent: 26.2,
		disk_mount: '/',
		containers_running: 2,
		apps_total: MOCK_APPS.length,
		apps_running: MOCK_APPS.filter((a) => a.status === 'running' || a.status === 'unhealthy')
			.length,
		collected_at: new Date().toISOString()
	};
}

// ── Resource Metrics ───────────────────────────────────────────────────────

const RANGE_SPAN: Record<MetricsRange, number> = {
	'1h': 3600,
	'24h': 86_400,
	'7d': 7 * 86_400,
	'30d': 30 * 86_400
};
const RANGE_STEP: Record<MetricsRange, number> = { '1h': 30, '24h': 180, '7d': 1500, '30d': 7200 };

/**
 * Deterministic-looking series: a daily wave with noise, a 20-minute outage
 * (gap) two thirds in, one crash-restart and one OOM kill. `empty` returns a
 * target that has never been measured.
 */
export function makeMockMetrics(range: string, empty = false): MetricsResponse {
	const r: MetricsRange = (['1h', '24h', '7d', '30d'] as const).includes(range as MetricsRange)
		? (range as MetricsRange)
		: '1h';
	const step = RANGE_STEP[r];
	const to = Math.floor(Date.now() / 1000);
	const from = Math.floor((to - RANGE_SPAN[r]) / step) * step;
	const series: MetricsResponse['series'] = {
		t: [],
		cpu: [],
		cpu_max: [],
		mem: [],
		mem_max: [],
		rx: [],
		tx: [],
		pids: []
	};
	if (empty) {
		return { range: r, step, from, to, mem_limit: null, series, events: [], current: null };
	}
	const n = Math.floor((to - from) / step);
	const gapStart = Math.floor(n * 0.66);
	const gapLen = Math.max(1, Math.round(1200 / step));
	for (let i = 0; i <= n; i++) {
		if (i >= gapStart && i < gapStart + gapLen) continue;
		const t = from + i * step;
		const wave = Math.sin((t / 86_400) * Math.PI * 2) * 0.5 + Math.sin(i / 7) * 0.3;
		const noise = Math.random();
		const cpu = Math.max(0.5, 18 + wave * 12 + noise * 6);
		const mem = 190_000_000 + wave * 35_000_000 + noise * 8_000_000;
		series.t.push(t);
		series.cpu.push(Math.round(cpu * 10) / 10);
		series.cpu_max.push(Math.round((cpu + 10 + noise * 30) * 10) / 10);
		series.mem.push(Math.round(mem));
		series.mem_max.push(Math.round(mem + 12_000_000));
		series.rx.push(Math.round(40_000 + wave * 25_000 + noise * 15_000));
		series.tx.push(Math.round(12_000 + wave * 6_000 + noise * 5_000));
		series.pids.push(14);
	}
	const gapAt = from + gapStart * step;
	const events: MetricsResponse['events'] = [
		{ t: gapAt + 30, kind: 'oom', detail: '1 process killed' },
		{ t: gapAt + gapLen * step, kind: 'restart', detail: 'exit code 137, out of memory' }
	];
	return {
		range: r,
		step,
		from,
		to,
		mem_limit: 512 * 1024 * 1024,
		series,
		events,
		current: {
			t: to,
			cpu: series.cpu.at(-1) ?? 0,
			mem: series.mem.at(-1) ?? 0,
			mem_peak: 301_000_000,
			rx: series.rx.at(-1) ?? 0,
			tx: series.tx.at(-1) ?? 0,
			pids: 14
		}
	};
}

// ── Deployments ────────────────────────────────────────────────────────────

/** ISO timestamp `minutes` ago (fixtures read as recent activity). */
function minutesAgo(minutes: number, plusSeconds = 0): string {
	return new Date(Date.now() - minutes * 60_000 + plusSeconds * 1000).toISOString();
}

/** A finished phase list: every visible step succeeded, `seconds` apart. */
function donePhases(start: string, steps: [DeployPhase, string | null, number][]): PhaseRecord[] {
	let t = new Date(start).getTime();
	return steps.map(([phase, label, secs]) => {
		if (label === null) return { phase, status: 'skipped' as const };
		const started_at = new Date(t).toISOString();
		t += secs * 1000;
		return {
			phase,
			status: 'succeeded' as const,
			label,
			started_at,
			finished_at: new Date(t).toISOString()
		};
	});
}

const GIT_STEPS: [DeployPhase, string | null, number][] = [
	['clone', 'Clone repository', 3],
	['build', 'Build image', 71],
	['start', 'Start container', 2],
	['health', 'Health check', 6],
	['release', 'Switch traffic', 4]
];

const DEP3_START = minutesAgo(42);
const DEP2_START = minutesAgo(60 * 26);
const DEP1_START = minutesAgo(60 * 50);
const FAILED_START = minutesAgo(18);

export const MOCK_DEPLOYMENTS: Record<string, Deployment[]> = {
	app_running: [
		{
			id: 'dep_3',
			app_id: 'app_running',
			image_tag: 'tond-api-service:3f2a1b9c1234',
			status: 'succeeded',
			trigger_source: 'webhook',
			trigger_detail: 'GitHub push to main',
			commit_sha: '3f2a1b9c12d4e5f60718293a4b5c6d7e8f901234',
			commit_message: 'Cache product queries for 60 s',
			commit_author: 'Ada Lovelace',
			has_generated_dockerfile: true,
			started_at: DEP3_START,
			finished_at: minutesAgo(42, 86),
			created_at: DEP3_START,
			duration_secs: 86,
			phases: donePhases(DEP3_START, GIT_STEPS),
			failure: null,
			image_pruned: false,
			is_current: true,
			rollback_available: false,
			rollback_unavailable_reason: 'This image is already running'
		},
		{
			id: 'dep_2',
			app_id: 'app_running',
			image_tag: 'tond-api-service:7f3a9b1d0e2c',
			status: 'succeeded',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '7f3a9b1d0e2c4b5a69788f7e6d5c4b3a29181716',
			commit_message: 'Add /v2/orders endpoint',
			commit_author: 'Grace Hopper',
			has_generated_dockerfile: true,
			started_at: DEP2_START,
			finished_at: minutesAgo(60 * 26, 94),
			created_at: DEP2_START,
			duration_secs: 94,
			phases: donePhases(DEP2_START, GIT_STEPS),
			failure: null,
			image_pruned: false,
			is_current: false,
			rollback_available: true,
			rollback_unavailable_reason: null
		},
		{
			id: 'dep_1b',
			app_id: 'app_running',
			image_tag: 'tond-api-service:c0ffee123456',
			status: 'succeeded',
			trigger_source: 'rollback',
			trigger_detail: 'admin@tond.local',
			rollback_of: 'dep_0',
			has_generated_dockerfile: false,
			started_at: minutesAgo(60 * 49),
			finished_at: minutesAgo(60 * 49, 12),
			created_at: minutesAgo(60 * 49),
			duration_secs: 12,
			image_pruned: true,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: "This deployment's image was pruned to save disk space"
		},
		{
			id: 'dep_1',
			app_id: 'app_running',
			image_tag: null,
			status: 'failed',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '1a2b3c4d5e6f708192a3b4c5d6e7f80910111213',
			commit_message: 'Upgrade to TypeScript 6',
			commit_author: 'Ada Lovelace',
			has_generated_dockerfile: true,
			started_at: DEP1_START,
			finished_at: minutesAgo(60 * 50, 48),
			created_at: DEP1_START,
			duration_secs: 48,
			phases: [
				{
					phase: 'clone',
					status: 'succeeded',
					label: 'Clone repository',
					started_at: DEP1_START,
					finished_at: minutesAgo(60 * 50, 3)
				},
				{
					phase: 'build',
					status: 'failed',
					label: 'Build image',
					started_at: minutesAgo(60 * 50, 3),
					finished_at: minutesAgo(60 * 50, 48)
				},
				{ phase: 'start', status: 'pending', label: 'Start container' },
				{ phase: 'health', status: 'pending', label: 'Health check' },
				{ phase: 'release', status: 'pending', label: 'Switch traffic' }
			],
			failure: {
				phase: 'build',
				code: 'build_failed',
				title: 'The build failed (exit code 1)',
				hint: 'The lines below are where the build stopped. Fix the error in your code or Dockerfile and deploy again.',
				detail: 'Docker build failed (exit code: exit status: 1)',
				excerpt: [
					'#12 [builder 5/6] RUN npm run build',
					"#12 4.211 src/orders/total.ts(14,7): error TS2322: Type 'string' is not assignable to type 'number'.",
					'#12 4.388 npm error Lifecycle script `build` failed with error: code 2',
					'#12 ERROR: process "/bin/sh -c npm run build" did not complete successfully: exit code: 2'
				]
			},
			image_pruned: false,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: 'Only successful deployments can be restored'
		}
	],
	app_failed: [
		{
			id: 'dep_wq_1',
			app_id: 'app_failed',
			image_tag: 'tond-worker-queue:9d8c7b6a5f4e',
			status: 'failed',
			trigger_source: 'manual',
			trigger_detail: 'admin@tond.local',
			commit_sha: '9d8c7b6a5f4e3d2c1b0a99887766554433221100',
			commit_message: 'Serve the dashboard with nginx',
			commit_author: 'Grace Hopper',
			has_generated_dockerfile: false,
			started_at: FAILED_START,
			finished_at: minutesAgo(18, 131),
			created_at: FAILED_START,
			duration_secs: 131,
			phases: [
				...donePhases(FAILED_START, GIT_STEPS.slice(0, 3)),
				{
					phase: 'health',
					status: 'failed',
					label: 'Wait for the app to listen',
					started_at: minutesAgo(18, 71),
					finished_at: minutesAgo(18, 131)
				},
				{ phase: 'release', status: 'pending', label: 'Switch traffic' }
			],
			failure: {
				phase: 'health',
				code: 'port_not_listening',
				title: 'Nothing answered on port 4000 within 60s',
				hint: "Tond expected the app to listen on port 4000 inside the container. If it uses another port (nginx listens on 80, many Node apps on 3000), change the app's port; make sure the app binds to 0.0.0.0, not localhost.",
				detail:
					'the new container did not become ready within 60s; the previous version keeps serving',
				excerpt: [
					'/docker-entrypoint.sh: Configuration complete; ready for start up',
					'2025/03/05 11:44:02 [notice] 1#1: nginx/1.27.3',
					'2025/03/05 11:44:02 [notice] 1#1: start worker processes'
				]
			},
			image_pruned: false,
			is_current: false,
			rollback_available: false,
			rollback_unavailable_reason: 'Only successful deployments can be restored'
		}
	]
};

/** Deploy log lines per fixture deployment (live mock deploys add their own). */
export const MOCK_DEPLOYMENT_LOGS: Record<string, DeploymentLogLine[]> = {
	dep_3: mockBuildLog(DEP3_START, 'Cache product queries for 60 s', true),
	dep_2: mockBuildLog(DEP2_START, 'Add /v2/orders endpoint', true),
	dep_1: mockBuildLog(DEP1_START, 'Upgrade to TypeScript 6', false),
	dep_wq_1: [
		{ level: 'info', message: 'Starting deployment...', ts: FAILED_START },
		{
			level: 'info',
			message: 'Commit 9d8c7b6 by Grace Hopper: Serve the dashboard with nginx',
			ts: minutesAgo(18, 2)
		},
		{ level: 'info', message: 'Building with Dockerfile', ts: minutesAgo(18, 3) },
		{
			level: 'info',
			message: '#8 naming to docker.io/library/tond-worker-queue:9d8c7b6a5f4e done',
			ts: minutesAgo(18, 66)
		},
		{ level: 'info', message: 'Starting new container on port 8104:4000', ts: minutesAgo(18, 67) },
		{
			level: 'info',
			message: 'Waiting up to 60s for the app to answer on port 4000',
			ts: minutesAgo(18, 71)
		},
		{
			level: 'error',
			message: 'Deployment failed: Nothing answered on port 4000 within 60s',
			ts: minutesAgo(18, 131)
		}
	]
};

/** A plausible TBuild/docker build log with ANSI colours. */
export function mockBuildLog(start: string, commit: string, ok: boolean): DeploymentLogLine[] {
	const t0 = new Date(start).getTime();
	const at = (s: number) => new Date(t0 + s * 1000).toISOString();
	const lines: [number, string, string][] = [
		[0, 'info', 'Starting deployment...'],
		[0, 'info', 'Using GitHub App installation token for clone'],
		[1, 'info', 'Cloning https://github.com/example/api-service.git (main)'],
		[3, 'info', `Commit 3f2a1b9 by Ada Lovelace: ${commit}`],
		[3, 'info', 'Building with TBuild (tond-scanner)'],
		[3, 'info', 'TBuild detection: stack=node profile=node-npm confidence=high'],
		[4, 'info', '#1 [internal] load build definition from Dockerfile'],
		[4, 'info', '#1 transferring dockerfile: 612B \u001b[32mdone\u001b[0m'],
		[5, 'info', '#5 [builder 1/6] FROM docker.io/library/node:20-alpine'],
		[9, 'info', '#9 [builder 3/6] RUN npm ci'],
		[31, 'info', '#9 21.87 added 412 packages, and audited 413 packages in 21s'],
		[31, 'info', '#9 21.88 \u001b[33m2 moderate severity vulnerabilities\u001b[0m'],
		[32, 'info', '#12 [builder 5/6] RUN npm run build'],
		[33, 'info', '#12 0.412 > api-service@2.3.0 build'],
		[33, 'info', '#12 0.413 > tsc -p tsconfig.build.json']
	];
	if (!ok) {
		lines.push(
			[
				47,
				'info',
				"#12 4.211 src/orders/total.ts(14,7): \u001b[31merror\u001b[0m \u001b[90mTS2322\u001b[0m: Type 'string' is not assignable to type 'number'."
			],
			[48, 'info', '#12 4.388 npm error Lifecycle script `build` failed with error: code 2'],
			[
				48,
				'info',
				'#12 \u001b[31mERROR\u001b[0m: process "/bin/sh -c npm run build" did not complete successfully: exit code: 2'
			],
			[48, 'error', 'Deployment failed: The build failed (exit code 1)']
		);
	} else {
		lines.push(
			[70, 'info', '#12 36.02 \u001b[32m✓\u001b[0m compiled 214 files'],
			[74, 'info', '#15 exporting to image'],
			[75, 'info', '#15 naming to docker.io/library/tond-api-service:3f2a1b9c1234 done'],
			[77, 'info', 'Starting new container on port 8107:8080'],
			[77, 'info', 'Waiting up to 30s for the health check on port 8080/healthz to pass'],
			[82, 'info', 'New container is ready — cutting over traffic'],
			[82, 'info', 'Waiting 3s for proxy to pick up new port before stopping old container'],
			[85, 'info', 'Draining old container 4be1c2a9d0f3 (30s grace)'],
			[86, 'info', 'Deployed successfully! Container: 9a8b7c6d5e4f']
		);
	}
	return lines.map(([s, level, message]) => ({ level, message, ts: at(s) }));
}
export const MOCK_GENERATED_DOCKERFILES: Record<string, string> = {
	dep_3: `FROM node:20-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM node:20-alpine
WORKDIR /app
COPY --from=builder /app .
EXPOSE 8080
CMD ["npm", "start"]`,
	dep_2: `FROM node:20-alpine AS builder
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html`
};

// ── Deploy Keys ────────────────────────────────────────────────────────────

export const MOCK_DEPLOY_KEYS: Record<string, DeployKey> = {
	app_running: {
		app_id: 'app_running',
		public_key: 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIM0mock+deploy+key+for+api+service tond@deploy'
	}
};

// ── Preview Deployments ────────────────────────────────────────────────────

export const MOCK_PREVIEWS: Record<string, PreviewDeployment[]> = {
	app_running: [
		{
			id: 'prev_1',
			app_id: 'app_running',
			project_preview_id: 'pprev_1',
			slug: 'feature-auth',
			subdomain: 'feature-auth-api-service.example-previews.com',
			container_id: 'c0ffee1234567890',
			host_port: 32100,
			image_tag: 'sha-preview1',
			ttl_seconds: 86400,
			status: 'running',
			expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
			url: 'https://feature-auth-api-service.example-previews.com',
			pr_number: 42,
			head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
			head_ref: 'feature/auth',
			pr_url: 'https://github.com/example/api-service/pull/42',
			comment_id: null
		},
		{
			id: 'prev_2',
			app_id: 'app_running',
			project_preview_id: null,
			slug: 'fix-memory-leak',
			subdomain: 'fix-memory-leak-api-service.example-previews.com',
			container_id: 'deadbeef12345678',
			host_port: 32101,
			image_tag: 'sha-preview2',
			ttl_seconds: 3600,
			status: 'expired',
			expires_at: new Date(Date.now() - 2 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 5 * 3_600_000).toISOString(),
			url: null,
			pr_number: null,
			head_sha: null,
			head_ref: null,
			pr_url: null,
			comment_id: null
		}
	]
};

// ── Project Previews ───────────────────────────────────────────────────────

export const MOCK_PROJECT_PREVIEWS: Record<string, ProjectPreview[]> = {
	proj_1: [
		{
			id: 'pprev_1',
			project_id: 'proj_1',
			slug: 'feature-auth',
			ttl_seconds: 86400,
			status: 'running',
			expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
			created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
			previews: [
				{
					id: 'prev_1',
					app_id: 'app_running',
					project_preview_id: 'pprev_1',
					slug: 'feature-auth',
					subdomain: 'feature-auth-api-service.example-previews.com',
					container_id: 'c0ffee1234567890',
					host_port: 32100,
					image_tag: 'sha-preview1',
					ttl_seconds: 86400,
					status: 'running',
					expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
					created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
					url: 'https://feature-auth-api-service.example-previews.com',
					pr_number: 42,
					head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
					head_ref: 'feature/auth',
					pr_url: 'https://github.com/example/api-service/pull/42',
					comment_id: null
				},
				{
					id: 'prev_3',
					app_id: 'app_stopped',
					project_preview_id: 'pprev_1',
					slug: 'feature-auth',
					subdomain: 'feature-auth-web-frontend.example-previews.com',
					container_id: 'abcdef0987654321',
					host_port: 32102,
					image_tag: 'sha-preview3',
					ttl_seconds: 86400,
					status: 'running',
					expires_at: new Date(Date.now() + 18 * 3_600_000).toISOString(),
					created_at: new Date(Date.now() - 6 * 3_600_000).toISOString(),
					url: 'https://feature-auth-web-frontend.example-previews.com',
					pr_number: 42,
					head_sha: 'a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2',
					head_ref: 'feature/auth',
					pr_url: 'https://github.com/example/web-frontend/pull/42',
					comment_id: null
				}
			]
		}
	],
	proj_2: []
};

// ── Alert Rules ────────────────────────────────────────────────────────────

export const MOCK_ALERT_RULES: AlertRule[] = [
	{
		id: 'alert_1',
		app_id: 'app_running',
		name: 'High CPU on api-service',
		metric: 'cpu',
		threshold: 80,
		webhook_url: 'https://hooks.example.com/cpu-alert',
		enabled: true,
		events: [],
		last_fired_at: '2025-03-06T04:12:00.000Z',
		created_at: '2025-01-20T00:00:00.000Z',
		updated_at: '2025-03-06T04:12:00.000Z'
	},
	{
		id: 'alert_2',
		app_id: null,
		name: 'Global Memory Usage',
		metric: 'memory',
		threshold: 85,
		webhook_url: 'https://hooks.example.com/memory-alert',
		enabled: false,
		events: [],
		last_fired_at: null,
		created_at: '2025-02-01T00:00:00.000Z',
		updated_at: '2025-02-01T00:00:00.000Z'
	},
	{
		id: 'alert_3',
		app_id: null,
		name: 'Deployment Notifications',
		metric: null,
		threshold: 0,
		webhook_url: '',
		enabled: true,
		events: ['deployment_success', 'deployment_failure'],
		last_fired_at: null,
		created_at: '2025-03-01T00:00:00.000Z',
		updated_at: '2025-03-01T00:00:00.000Z'
	}
];

// ── Audit Log ──────────────────────────────────────────────────────────────

type AuditSeed = Omit<AuditLogEntry, 'seq' | 'created_at' | 'outcome' | 'ip'> & {
	minutesAgo: number;
	outcome?: string;
	ip?: string | null;
};

const user = (email: string, id: string) => ({
	actor_kind: 'user',
	actor_id: id,
	actor_label: email
});
const ADMIN = user('admin@localhost', 'usr_dev');
const DANA = user('dana@example.com', 'usr_2');
const app = (id: string, name: string) => ({
	resource_type: 'app',
	resource_id: id,
	resource_name: name
});
const none = { resource_type: null, resource_id: null, resource_name: null };

const AUDIT_SEEDS: AuditSeed[] = [
	{
		...ADMIN,
		...app('app_running', 'api-service'),
		action: 'deploy',
		detail: null,
		minutesAgo: 2,
		ip: '198.51.100.7'
	},
	{
		actor_kind: 'token',
		actor_id: 'tok_agent',
		actor_label: 'claude-ops-agent',
		...app('app_running', 'api-service'),
		action: 'restart',
		detail: null,
		minutesAgo: 9,
		ip: '203.0.113.20'
	},
	{
		actor_kind: 'anonymous',
		actor_id: null,
		actor_label: 'root@example.com',
		...none,
		action: 'login_failed',
		detail: 'wrong email or password',
		minutesAgo: 14,
		outcome: 'failure',
		ip: '185.220.101.3'
	},
	{
		actor_kind: 'token',
		actor_id: 'tok_ci',
		actor_label: 'github-actions-web',
		...app('app_running', 'api-service'),
		action: 'deploy',
		detail: null,
		minutesAgo: 42,
		ip: '140.82.112.4'
	},
	{
		...ADMIN,
		resource_type: 'token',
		resource_id: 'tok_agent',
		resource_name: 'claude-ops-agent',
		action: 'token_create',
		detail: 'kind=agent scopes=apps:read,apps:deploy,apps:control projects=proj_1 expires=in 24h',
		minutesAgo: 19 * 60,
		ip: '198.51.100.7'
	},
	{
		...DANA,
		...app('app_running', 'api-service'),
		action: 'env_update',
		detail: 'set=LOG_LEVEL',
		minutesAgo: 3 * 60,
		ip: '192.0.2.44'
	},
	{
		...ADMIN,
		resource_type: 'user',
		resource_id: 'usr_3',
		resource_name: 'victor@example.com',
		action: 'user_invite',
		detail: 'role=viewer',
		minutesAgo: 26 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		resource_type: 'user',
		resource_id: 'usr_4',
		resource_name: 'former@example.com',
		action: 'user_disable',
		detail: null,
		minutesAgo: 30 * 60,
		ip: '198.51.100.7'
	},
	{
		...DANA,
		resource_type: 'user',
		resource_id: 'usr_2',
		resource_name: 'dana@example.com',
		action: 'login',
		detail: 'with 2FA',
		minutesAgo: 3 * 60 + 5,
		ip: '192.0.2.44'
	},
	{
		actor_kind: 'local_socket',
		actor_id: null,
		actor_label: 'local-socket',
		resource_type: 'settings',
		resource_id: null,
		resource_name: null,
		action: 'host_terminal_enabled',
		detail: null,
		minutesAgo: 50 * 60
	},
	{
		...ADMIN,
		...app('app_stopped', 'web-frontend'),
		action: 'stop',
		detail: null,
		minutesAgo: 52 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		resource_type: 'settings',
		resource_id: null,
		resource_name: null,
		action: 'settings_update',
		detail: 'session_idle_timeout_hours=12',
		minutesAgo: 70 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		...app('app_running', 'api-service'),
		action: 'rollback',
		detail: 'deployment=dep_2 image_tag=tond-api-service:7f3a9b1c2d4e',
		minutesAgo: 90 * 60,
		ip: '198.51.100.7'
	},
	{
		...ADMIN,
		...app('app_stopped', 'web-frontend'),
		action: 'create',
		detail: null,
		minutesAgo: 200 * 60,
		ip: '198.51.100.7'
	}
];

export const MOCK_AUDIT_LOG: AuditLogEntry[] = [...AUDIT_SEEDS]
	.sort((a, b) => a.minutesAgo - b.minutesAgo)
	.map((seed, i) => {
		const { minutesAgo, outcome, ip, ...rest } = seed;
		return {
			...rest,
			seq: AUDIT_SEEDS.length - i + 40,
			created_at: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
			outcome: outcome ?? 'success',
			ip: ip ?? null
		};
	});

// ── Managed Databases (Phase 6.3) ──────────────────────────────────────────

export const MOCK_DATABASES: ManagedDatabase[] = [
	{
		id: 'db_pg1',
		name: 'app-postgres',
		db_type: 'postgres',
		project_id: 'proj_1',
		host_port: 5401,
		status: 'running',
		db_name: 'app_db',
		db_user: 'app_user',
		db_password: 'mock_password_1',
		root_password: null,
		memory_limit: 268435456,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'postgres:16-alpine',
		custom_image: null,
		created_at: '2025-02-01T10:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_redis1',
		name: 'cache-redis',
		db_type: 'redis',
		project_id: null,
		host_port: 5402,
		status: 'running',
		db_name: null,
		db_user: 'default',
		db_password: 'mock_redis_password',
		root_password: null,
		memory_limit: null,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'redis:7-alpine',
		custom_image: null,
		created_at: '2025-02-10T14:00:00.000Z',
		updated_at: '2025-02-10T14:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_mysql1',
		name: 'legacy-mysql',
		db_type: 'mysql',
		project_id: null,
		host_port: null,
		status: 'stopped',
		db_name: 'legacy_db',
		db_user: 'legacy_user',
		db_password: 'mock_password_2',
		root_password: 'mock_root_password_mysql',
		memory_limit: 536870912,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'mysql:8',
		custom_image: null,
		created_at: '2025-01-20T08:00:00.000Z',
		updated_at: '2025-02-28T12:00:00.000Z',
		public_access: false
	},
	{
		id: 'db_mariadb1',
		name: 'analytics-mariadb',
		db_type: 'mariadb',
		project_id: 'proj_2',
		host_port: 5403,
		status: 'running',
		db_name: 'analytics_db',
		db_user: 'analytics_user',
		db_password: 'mock_mariadb_password',
		root_password: 'mock_root_password_mariadb',
		memory_limit: 268435456,
		custom_external_url: null,
		custom_internal_url: null,
		image_used: 'mariadb:11',
		custom_image: null,
		created_at: '2025-03-01T12:00:00.000Z',
		updated_at: '2025-03-01T12:00:00.000Z',
		public_access: true
	}
];

// ── Cron Jobs (Phase 6.4) ──────────────────────────────────────────────────

export const MOCK_CRON_JOBS: CronJob[] = [
	{
		id: 'cron_1',
		name: 'hourly-report',
		schedule: '0 * * * *',
		image: 'curlimages/curl:latest',
		command: '["curl", "https://example.com/report"]',
		project_id: 'proj_1',
		enabled: true,
		last_run_at: new Date(Date.now() - 45 * 60_000).toISOString(),
		next_run_at: new Date(Date.now() + 15 * 60_000).toISOString(),
		status: 'idle',
		created_at: '2025-02-01T10:00:00.000Z',
		updated_at: new Date(Date.now() - 45 * 60_000).toISOString(),
		timeout_seconds: 60,
		env: { REPORT_URL: 'https://example.com/report', API_KEY: 'secret123' },
		last_run_status: 'success',
		concurrency_policy: 'skip',
		memory_mb: 128,
		cpu_shares: null,
		timezone: 'America/New_York',
		max_retries: 2,
		retry_delay_seconds: 30,
		volumes: null
	},
	{
		id: 'cron_2',
		name: 'nightly-backup',
		schedule: '0 2 * * *',
		image: 'alpine:latest',
		command: '["sh", "-c", "echo backup done"]',
		project_id: 'proj_2',
		enabled: true,
		last_run_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
		next_run_at: new Date(Date.now() + 2 * 3_600_000).toISOString(),
		status: 'idle',
		created_at: '2025-01-15T08:00:00.000Z',
		updated_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
		timeout_seconds: 300,
		env: null,
		last_run_status: 'success',
		concurrency_policy: 'skip',
		memory_mb: 256,
		cpu_shares: 512,
		timezone: null,
		max_retries: 0,
		retry_delay_seconds: 60,
		volumes: [
			{
				name: 'backups',
				host_path: '/opt/tond/data/volumes/_cron/cron_2/backups',
				container_path: '/backup'
			}
		]
	},
	{
		id: 'cron_3',
		name: 'weekly-cleanup',
		schedule: '0 0 * * 0',
		image: 'alpine:latest',
		command: null,
		project_id: null,
		enabled: false,
		last_run_at: null,
		next_run_at: null,
		status: 'idle',
		created_at: '2025-03-01T09:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z',
		timeout_seconds: null,
		env: null,
		last_run_status: null,
		concurrency_policy: 'skip',
		memory_mb: null,
		cpu_shares: null,
		timezone: null,
		max_retries: 0,
		retry_delay_seconds: 60,
		volumes: null
	}
];

export const MOCK_CRON_RUNS: Record<string, CronJobRun[]> = {
	cron_1: [
		{
			id: 'run_1_3',
			cron_job_id: 'cron_1',
			status: 'success',
			exit_code: 0,
			output: 'HTTP/2 200\ncontent-type: application/json\n{"ok":true}',
			started_at: new Date(Date.now() - 45 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 44 * 60_000 - 8_000).toISOString(),
			attempt: 1
		},
		{
			id: 'run_1_2',
			cron_job_id: 'cron_1',
			status: 'success',
			exit_code: 0,
			output: 'HTTP/2 200',
			started_at: new Date(Date.now() - 105 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 104 * 60_000 - 5_000).toISOString(),
			attempt: 2
		},
		{
			id: 'run_1_1',
			cron_job_id: 'cron_1',
			status: 'failed',
			exit_code: 6,
			output: 'curl: (6) Could not resolve host: example.com',
			started_at: new Date(Date.now() - 165 * 60_000).toISOString(),
			finished_at: new Date(Date.now() - 164 * 60_000 - 3_000).toISOString(),
			attempt: 1
		}
	],
	cron_2: [
		{
			id: 'run_2_1',
			cron_job_id: 'cron_2',
			status: 'success',
			exit_code: 0,
			output: 'backup done',
			started_at: new Date(Date.now() - 22 * 3_600_000).toISOString(),
			finished_at: new Date(Date.now() - 22 * 3_600_000 + 2_000).toISOString(),
			attempt: 1
		}
	],
	cron_3: []
};

// ── Registry Credentials (Phase 4.3) ──────────────────────────────────────

export const MOCK_REGISTRY_CREDENTIALS: RegistryCredential[] = [
	{
		id: 'cred_ghcr',
		name: 'GitHub Container Registry',
		server: 'ghcr.io',
		username: 'myorg',
		created_at: '2025-01-20T10:00:00.000Z',
		updated_at: '2025-01-20T10:00:00.000Z'
	},
	{
		id: 'cred_dockerhub',
		name: 'Docker Hub',
		server: 'registry-1.docker.io',
		username: 'mydockerhubuser',
		created_at: '2025-02-05T14:00:00.000Z',
		updated_at: '2025-02-05T14:00:00.000Z'
	},
	{
		id: 'cred_gitlab',
		name: 'GitLab Registry',
		server: 'registry.gitlab.com',
		username: 'deploy-token-123',
		created_at: '2025-03-01T09:00:00.000Z',
		updated_at: '2025-03-01T09:00:00.000Z'
	}
];

// ── GitHub Apps (Phase 4.4) ────────────────────────────────────────────────

export const MOCK_GITHUB_APPS: GitHubApp[] = [
	{
		id: 'ghapp_1',
		name: 'tond-dev',
		app_id: 123456,
		client_id: 'Iv1.abc123def456',
		slug: 'tond-dev',
		installation_id: 78901234,
		installed: true,
		created_at: '2025-03-01T00:00:00.000Z',
		updated_at: '2025-03-01T00:00:00.000Z'
	}
];

// ── GitHub Repos + Branches (Phase 4.4) ───────────────────────────────────

export const MOCK_GITHUB_REPOS: Record<string, GitHubRepo[]> = {
	ghapp_1: [
		{
			id: 101,
			full_name: 'example/api-service',
			name: 'api-service',
			private: false,
			clone_url: 'https://github.com/example/api-service.git',
			ssh_url: 'git@github.com:example/api-service.git',
			default_branch: 'main'
		},
		{
			id: 102,
			full_name: 'example/web-frontend',
			name: 'web-frontend',
			private: false,
			clone_url: 'https://github.com/example/web-frontend.git',
			ssh_url: 'git@github.com:example/web-frontend.git',
			default_branch: 'main'
		},
		{
			id: 103,
			full_name: 'example/worker-queue',
			name: 'worker-queue',
			private: true,
			clone_url: 'https://github.com/example/worker-queue.git',
			ssh_url: 'git@github.com:example/worker-queue.git',
			default_branch: 'main'
		},
		{
			id: 104,
			full_name: 'example/ml-inference',
			name: 'ml-inference',
			private: true,
			clone_url: 'https://github.com/example/ml-inference.git',
			ssh_url: 'git@github.com:example/ml-inference.git',
			default_branch: 'develop'
		}
	]
};

export const MOCK_GITHUB_BRANCHES: Record<string, GitHubBranch[]> = {
	'example/api-service': [
		{ name: 'main' },
		{ name: 'develop' },
		{ name: 'feature/auth' },
		{ name: 'fix/memory-leak' }
	],
	'example/web-frontend': [{ name: 'main' }, { name: 'staging' }, { name: 'feature/dark-mode' }],
	'example/worker-queue': [{ name: 'main' }, { name: 'queue-v2' }],
	'example/ml-inference': [{ name: 'develop' }, { name: 'main' }, { name: 'experiment/llm' }]
};

// ── Notification Integrations ──────────────────────────────────────────────

export const MOCK_INTEGRATIONS: Integration[] = [
	{
		id: 'int_telegram_1',
		name: 'Ops Telegram',
		kind: 'telegram',
		config: { bot_token: '123456789:AABBccDDeeffGGhhIIjjKKll', chat_id: '-1001234567890' },
		enabled: true,
		created_at: '2025-03-01T10:00:00.000Z',
		updated_at: '2025-03-01T10:00:00.000Z'
	},
	{
		id: 'int_discord_1',
		name: 'Discord #alerts',
		kind: 'discord',
		config: { webhook_url: 'https://discord.com/api/webhooks/1234567890/mock-token' },
		enabled: true,
		created_at: '2025-03-05T12:00:00.000Z',
		updated_at: '2025-03-05T12:00:00.000Z'
	},
	{
		id: 'int_slack_1',
		name: 'Slack #incidents',
		kind: 'slack',
		config: { webhook_url: 'https://hooks.slack.com/services/T00/B00/mock-token' },
		enabled: false,
		created_at: '2025-03-10T08:00:00.000Z',
		updated_at: '2025-03-10T08:00:00.000Z'
	},
	{
		id: 'int_email_1',
		name: 'Team Email',
		kind: 'email',
		config: {
			from_name: 'Tond Alerts',
			from_addr: 'alerts@example.com',
			smtp_host: 'smtp.example.com',
			smtp_port: 587,
			smtp_user: 'alerts@example.com',
			smtp_password: 'smtp-mock-password',
			smtp_encryption: 'starttls',
			to_addrs: ['ops@example.com', 'admin@example.com']
		},
		enabled: true,
		created_at: '2025-03-12T09:00:00.000Z',
		updated_at: '2025-03-12T09:00:00.000Z'
	}
];

// ── Storage Providers ──────────────────────────────────────────────────────

export const MOCK_STORAGE_PROVIDERS: StorageProvider[] = [
	{
		id: 'sp_r2_1',
		name: 'Cloudflare R2 (prod)',
		kind: 's3',
		config: {
			endpoint: 'https://abc123.r2.cloudflarestorage.com',
			region: 'auto',
			bucket: 'tond-backups',
			prefix: 'prod/',
			access_key_id: 'r2key123abc'
		},
		created_at: '2025-03-01T10:00:00.000Z',
		updated_at: '2025-03-01T10:00:00.000Z'
	},
	{
		id: 'sp_s3_1',
		name: 'AWS S3 (offsite-dr)',
		kind: 's3',
		config: {
			endpoint: null,
			region: 'eu-west-1',
			bucket: 'tond-dr-backups',
			prefix: null,
			access_key_id: 'AKIAIOSFODNN7EXAMPLE'
		},
		created_at: '2025-03-10T14:00:00.000Z',
		updated_at: '2025-03-10T14:00:00.000Z'
	}
];

// ── Snapshots ──────────────────────────────────────────────────────────────

export const MOCK_SNAPSHOTS: Snapshot[] = [
	{
		id: 'snap_1',
		name: 'Scheduled 2026-03-31 03:00',
		status: 'completed',
		trigger: 'scheduled',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(new Date().setHours(3, 0, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(3, 4, 0, 0)).toISOString(),
		delta_bytes: 2_202_009,
		total_bytes: 98_304_000,
		local_path: '/opt/tond/snapshots/snap_1',
		remote_key: 'prod/snap_1.tar.gz',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'abc123def456',
		error_message: null,
		notes: null
	},
	{
		id: 'snap_2',
		name: 'Pre-deploy: api-service',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:api-service',
		trigger_app: 'api-service',
		deployment_id: 'dep_3',
		parent_snapshot_id: 'snap_1',
		created_at: new Date(new Date().setHours(11, 42, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(11, 42, 8, 0)).toISOString(),
		delta_bytes: 4_408,
		total_bytes: 1_048_576,
		local_path: '/opt/tond/snapshots/snap_2',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null
	},
	{
		id: 'snap_3',
		name: 'Pre-deploy: web-frontend',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:web-frontend',
		trigger_app: 'web-frontend',
		deployment_id: 'dep_2',
		parent_snapshot_id: 'snap_1',
		created_at: new Date(new Date().setHours(14, 15, 0, 0)).toISOString(),
		completed_at: new Date(new Date().setHours(14, 15, 5, 0)).toISOString(),
		delta_bytes: 1_153,
		total_bytes: 524_288,
		local_path: '/opt/tond/snapshots/snap_3',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null
	},
	{
		id: 'snap_4',
		name: 'Scheduled 2026-03-30 03:00',
		status: 'completed',
		trigger: 'scheduled',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(Date.now() - 24 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 240_000).toISOString(),
		delta_bytes: 1_887_436,
		total_bytes: 96_102_400,
		local_path: '/opt/tond/snapshots/snap_4',
		remote_key: 'prod/snap_4.tar.gz',
		storage_provider_id: 'sp_r2_1',
		checksum_sha256: 'def456ghi789',
		error_message: null,
		notes: null
	},
	{
		id: 'snap_5',
		name: 'Pre-deploy: api-service',
		status: 'completed',
		trigger: 'pre_deploy',
		scope: 'app:api-service',
		trigger_app: 'api-service',
		deployment_id: 'dep_2',
		parent_snapshot_id: 'snap_4',
		created_at: new Date(Date.now() - 24 * 3_600_000 + 6 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 6 * 3_600_000 + 6000).toISOString(),
		delta_bytes: 3_983,
		total_bytes: 983_040,
		local_path: '/opt/tond/snapshots/snap_5',
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: null,
		notes: null
	},
	{
		id: 'snap_failed',
		name: 'Manual',
		status: 'failed',
		trigger: 'manual',
		scope: 'full',
		trigger_app: null,
		deployment_id: null,
		parent_snapshot_id: null,
		created_at: new Date(Date.now() - 24 * 3_600_000 + 12 * 3_600_000).toISOString(),
		completed_at: new Date(Date.now() - 24 * 3_600_000 + 12 * 3_600_000 + 5000).toISOString(),
		delta_bytes: null,
		total_bytes: null,
		local_path: null,
		remote_key: null,
		storage_provider_id: null,
		checksum_sha256: null,
		error_message: 'Disk write error: no space left on device',
		notes: null
	}
];

// ── Backup Config ──────────────────────────────────────────────────────────

export const MOCK_BACKUP_CONFIG: BackupConfig = {
	enabled: true,
	full_schedule_cron: '0 3 * * *',
	full_retention_count: 7,
	pre_deploy_enabled: true,
	pre_deploy_retention_per_app: 5,
	keep_local: true,
	storage_provider_id: 'sp_r2_1'
};
